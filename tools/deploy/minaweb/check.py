#!/usr/bin/env python3
"""External launch gate: HTTPS/headers, CSP and framing, and for each relay
its routes, credential service, STUN and native peers; then direct and TURN
sync with every relay connected.

    check.py OUT --relay https://relay1.minaweb.org --turn-ip IPV4_1 \\
        --relay-http http://127.0.0.1:13000 [--relay ... --turn-ip ... \\
        --relay-http ...]

One relay may still give its operator API as RELAY_HTTP. --relay-only checks
the relays alone, before a page is deployed: their routes, credential
services (CORS for --site), STUN and native peers. CHROME and
PUPPETEER_DIR select the browser. The operator APIs are SSH-forwarded;
never expose them publicly. Evidence goes to the output dir.
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
import sys
import time
import hashlib
import ipaddress
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from urllib.parse import urljoin

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "regression/webnode"))
import csp  # noqa: E402


# Cloudflare's Browser Integrity Check refuses urllib's default User-Agent
# (403, error code 1010) on Pages and on the R2 custom domain.
USER_AGENT = "minaweb-check/1"


def get(url, method="GET", headers=None):
    try:
        with urlopen(Request(url, method=method,
                             headers={"User-Agent": USER_AGENT, **(headers or {})}), timeout=15) as r:
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


# Sources no directive of the page's policy may hold.
UNSAFE_SOURCES = {"'unsafe-inline'", "'unsafe-eval'", "'unsafe-hashes'", "*", "data:",
                  "http:", "https:", "filesystem:"}


def build_relays(build):
    """The relays a build dials: build-info.json's relays, or an older
    build's one relay."""
    relays = build.get("relays") or [build["relay"]]
    assert isinstance(relays, list) and relays and all(isinstance(r, str) for r in relays), "build-info.json names no relays"
    return relays


def check_page_headers(headers, build, relays):
    """The page's Content-Security-Policy, framing and feature headers: the
    policy exactly as the build made it and as this gate makes it again
    from the build's relays, WASM and circuit hosts and the credential
    services (the relays), and strict whatever it says."""
    if isinstance(relays, str):
        relays = [relays]
    expected = csp.policy(build_relays(build), build["wasm"]["path"], build["circuitsUrl"],
                          [csp.origin(r) for r in relays])
    served = headers.get("Content-Security-Policy")
    assert served, "no Content-Security-Policy"
    assert headers.get_all("Content-Security-Policy") == [served], "more than one Content-Security-Policy"
    assert build.get("contentSecurityPolicy") == expected, "build-info.json's policy is not this build's"
    assert served == expected, "the served Content-Security-Policy is not the build's"
    directives = csp.parse(served)
    for name, sources in {
        "default-src": ["'none'"], "script-src": ["'self'", "'wasm-unsafe-eval'"],
        "object-src": ["'none'"], "base-uri": ["'none'"], "form-action": ["'none'"],
        "frame-ancestors": ["'none'"],
    }.items():
        assert directives.get(name) == sources, f"CSP {name} is not {' '.join(sources)}"
    for name, sources in directives.items():
        assert not UNSAFE_SOURCES & {s.lower() for s in sources}, f"CSP {name} allows too much"
    assert headers.get("X-Frame-Options") == csp.X_FRAME_OPTIONS, "X-Frame-Options"
    assert headers.get("Permissions-Policy") == csp.PERMISSIONS_POLICY, "Permissions-Policy"
    assert headers.get("X-Content-Type-Options") == "nosniff", "X-Content-Type-Options"


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


def ice_hosts(ice):
    """The hosts of a list's TURN URLs."""
    return {re.fullmatch(r"turns?:(\[[^]]+\]|[^:?]+).*", u)[1]
            for server in ice for u in ([server["urls"]] if isinstance(server["urls"], str) else server["urls"])
            if u.startswith(("turn:", "turns:"))}


def check_credentials(ice, now=None, turn_ip=None):
    """A credential service's list: valid URLs, expiring TURN credentials
    and, given turn_ip, TURN on that address only (each relay's coturn
    forwards to its own relay)."""
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
    if turn_ip is not None:
        assert ice_hosts(ice) == {turn_ip}, "TURN is not on this relay's address"


def check_browser_run(log, mode, relays, tip):
    """A run.mjs log: a sample where the page was Synced within one block of
    the relays' tip with a peer for each relay, and an open WebRTC
    connection to each relay's address (through TURN in the turn mode,
    where each relay's TURN reaches that relay only)."""
    assert "crossOriginIsolated=true" in log, f"{mode}: not cross-origin isolated"
    kind = "relay" if mode == "turn" else r"\w+"
    for line in log.splitlines():
        found = re.search(r"status=(\{.*?\}) rtc=(\{.*\})$", line)
        if not found:
            continue
        status, rtc = json.loads(found[1]), json.loads(found[2])
        pairs = rtc.get("pairs") or []
        if (status.get("sync") == "Synced" and (status.get("peers") or 0) >= len(relays)
                and abs(tip - (status.get("best_tip") or 0)) <= 1
                and all(any(re.fullmatch(rf"{kind}>\w+:{re.escape(ip)}:\d+", pair) for pair in pairs)
                        for ip in relays)):
            return
    raise AssertionError(f"{mode}: no sample synced to the current tip with every relay connected"
                         + (" through its TURN" if mode == "turn" else ""))


def check_relay(site, relay, turn_ip, turn_port, native_url):
    """One relay's public routes, credential service, STUN and native node."""
    for route in ("healthz", "readyz"):
        assert get(relay + "/" + route)[0] == 200, f"{relay}: {route}"
    for route in ("status", "graphql", "state", "metrics"):
        assert get(relay + "/" + route)[0] == 404, f"{relay}: /{route} is exposed"
    code, _, _ = get(relay + "/mina/webrtc/signal/x")
    assert code == 400, f"{relay}: signalling malformed-offer gate: {code}"
    code, _, _ = get(relay + "/mina/webrtc/signal", "OPTIONS", {
        "Origin": site, "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "content-type"})
    assert code in (200, 204), f"{relay}: signalling preflight: {code}"
    # Credentials are deliberately neither logged nor saved in evidence.
    code, h, body = get(relay + "/ice-servers.json", headers={"Origin": site})
    assert code == 200 and h.get("Cache-Control") == "no-store", f"{relay}: credential service"
    assert h.get("Access-Control-Allow-Origin") == site, f"{relay}: credential CORS"
    check_credentials(json.loads(body), turn_ip=turn_ip)
    tx = secrets.token_bytes(12)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(8)
        sock.connect((turn_ip, turn_port))
        sock.send(struct.pack("!HHI", 1, 0, 0x2112A442) + tx)
        response = sock.recv(2048)
    assert len(response) >= 20 and response[8:20] == tx, f"{turn_ip}: STUN"
    assert struct.unpack("!H", response[:2])[0] == 0x101, f"{turn_ip}: STUN"
    native = json.loads(get(native_url + "/status")[2])
    connected = [x for x in native["peers"] if x["connection_status"] == "Connected" and x["is_libp2p"]]
    assert native["network"] == "devnet" and connected, f"{relay}: no native devnet peer connections"
    assert native["transition_frontier"]["sync"]["status"] == "Synced", f"{relay}: not synced"
    return {"relay": relay, "nativeConnectedPeers": len(connected), "stun": "passed",
            "tip": native["transition_frontier"]["best_tip"]["height"]}


def native_tip(native_urls):
    return max(json.loads(get(u + "/status")[2])["transition_frontier"]["best_tip"]["height"]
               for u in native_urls)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("output", type=Path)
    p.add_argument("--site", default="https://minaweb.org")
    p.add_argument("--relay", action="append", required=True,
                   help="a relay's HTTPS origin, such as https://relay1.minaweb.org, once a "
                        "relay, in the order of --turn-ip and --relay-http")
    p.add_argument("--turn-ip", action="append", required=True,
                   help="each relay's public IPv4, its TURN and STUN address")
    p.add_argument("--relay-http", action="append", default=[],
                   help="each relay's SSH-forwarded operator API (default RELAY_HTTP, for one relay)")
    p.add_argument("--turn-port", type=int, default=3478)
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--automated-only", action="store_true",
                      help="run HTTP, credential, STUN and native checks; browser sync remains a separate gate")
    mode.add_argument("--relay-only", action="store_true",
                      help="check only the relays, before the page is deployed; writes RELAY-PASS")
    args = p.parse_args()
    relays = [csp.origin(r) for r in args.relay]
    native_urls = [u.rstrip("/") for u in (args.relay_http or
                   [u for u in os.environ.get("RELAY_HTTP", "").split(",") if u])]
    if not len(relays) == len(args.turn_ip) == len(native_urls):
        p.error("give each relay its --relay, --turn-ip and --relay-http (or RELAY_HTTP), in order")
    if len(set(relays)) != len(relays) or len(set(args.turn_ip)) != len(args.turn_ip):
        p.error("a relay or TURN address is given twice")
    turn_ips = [str(ipaddress.IPv4Address(ip)) for ip in args.turn_ip]
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    if args.relay_only:
        preflight = [check_relay(args.site, relay, ip, args.turn_port, native)
                     for relay, ip, native in zip(relays, turn_ips, native_urls)]
        (out / "relays.json").write_text(json.dumps(preflight, indent=2) + "\n")
        (out / "RELAY-PASS").write_text(
            f"routes, credential service, STUN and native peers passed for {len(relays)} relays; "
            "page not checked\n")
        return
    code, headers, _ = get(args.site)
    assert code == 200 and headers.get("Cross-Origin-Opener-Policy") == "same-origin"
    assert headers.get("Cross-Origin-Embedder-Policy") == "require-corp"
    code, _, body = get(args.site + "/build-info.json")
    assert code == 200
    build = json.loads(body)
    assert build["network"] == "devnet" and build["wasm"]["pinnedInPage"]
    # The page dials exactly the relays given here.
    assert sorted(csp.relay_origin(r) for r in build_relays(build)) == sorted(relays), \
        "the build's relays are not the ones given"
    # The page and a route the host answers with it.
    check_page_headers(headers, build, relays)
    code, route_headers, _ = get(args.site + "/blocks")
    assert code == 200, f"/blocks: {code}"
    check_page_headers(route_headers, build, relays)
    check_assets(build, args.site)
    preflight = [check_relay(args.site, relay, ip, args.turn_port, native)
                 for relay, ip, native in zip(relays, turn_ips, native_urls)]
    # The page's ICE lists, as it fetches them (a relay's credential service
    # directly, or an older build's redirect), carry every relay's TURN.
    urls = build.get("iceServers")
    urls = [urls] if isinstance(urls, str) else urls or []
    hosts = set()
    for url in urls:
        code, h, body = get(urljoin(args.site + "/", url), headers={"Origin": args.site})
        assert code == 200 and h.get("Cache-Control") == "no-store", f"ICE list {url}"
        ice = json.loads(body)
        check_credentials(ice)
        hosts |= ice_hosts(ice)
    assert hosts == set(turn_ips), "the page's ICE lists do not carry each relay's TURN"
    (out / "preflight.json").write_text(json.dumps({"build": build, "relays": preflight}, indent=2) + "\n")
    if args.automated_only:
        (out / "AUTOMATED-PASS").write_text(
            f"HTTP assets, credential expiry, STUN and native peers passed for {len(relays)} "
            "relays; browser sync not run\n")
        return
    for mode in ("direct", "turn"):
        run = out / mode
        env = dict(os.environ, BASE=args.site, PAGE="/", BUTTON="Start the web node in this tab", OUT=str(run),
                   MIN_SYNC_HEIGHT=str(native_tip(native_urls) - 1),
                   EVERY_MS="5000", UNTIL_SYNCED="30", ICE_POLICY="relay" if mode == "turn" else "")
        subprocess.run(["node", str(HERE.parents[1] / "regression/webnode/run.mjs"), "15"],
                       env=env, check=True)
        check_browser_run((run / "run.log").read_text(), mode, turn_ips, native_tip(native_urls))
    (out / "PASS").write_text(
        f"HTTPS, STUN, native peers, direct sync and TURN sync passed for {len(relays)} relays\n")


if __name__ == "__main__":
    main()
