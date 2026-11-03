#!/usr/bin/env python3
"""Offline regression gates for public release preflight; no deployment access."""
import hashlib
import importlib.util
import json
import os
from email.message import Message
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


def module(name):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


check = module("check")
release = module("release")
issuer = module("turn-credentials")
encoding_check = module("check-wasm-encoding")
guard = module("turn-egress")
csp = check.csp

PEER = "12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ"
PEER2 = "12D3KooWRhfZ3wQKx4LWJp4xc45UYoT6UqASoLKgpx1bE52MjSLd"
NATIVE = {"network": "devnet", "peers": [{"connection_status": "Connected", "is_libp2p": True}],
          "transition_frontier": {"sync": {"status": "Synced"}, "best_tip": {"height": 500}}}
RELEASE = "https://assets.minaweb.org/releases/" + "c" * 40 + "/" + "e" * 32


def public_build():
    """build-info.json as main's single-relay release.py wrote it (one relay,
    ICE servers through the site's redirect), policy included."""
    build = {"network": "devnet", "relay": f"/dns4/relay.minaweb.org/tcp/443/webrtc/https/p2p/{PEER}",
             "iceServers": "/webnode-devnet/ice-servers.json",
             "wasm": {"path": RELEASE + "/wasm/mina_node_web_bg.0123456789abcdef.wasm", "pinnedInPage": True},
             "circuitsUrl": RELEASE + "/circuit-blobs"}
    build["contentSecurityPolicy"] = csp.policy(
        [build["relay"]], build["wasm"]["path"], build["circuitsUrl"], ["https://relay.minaweb.org"])
    return build


def two_relay_build():
    """build-info.json as release.py builds it for relay1 and relay2."""
    relays = [f"/dns4/relay{n}.minaweb.org/tcp/443/webrtc/https/p2p/{peer}"
              for n, peer in ((1, PEER), (2, PEER2))]
    ice = [f"https://relay{n}.minaweb.org/ice-servers.json" for n in (1, 2)]
    build = {"network": "devnet", "relays": relays, "iceServers": ice,
             "wasm": {"path": RELEASE + "/wasm/mina_node_web_bg.0123456789abcdef.wasm", "pinnedInPage": True},
             "circuitsUrl": RELEASE + "/circuit-blobs"}
    build["contentSecurityPolicy"] = csp.policy(relays, build["wasm"]["path"], build["circuitsUrl"], ice)
    return build


class Stun:
    def __enter__(self): return self
    def __exit__(self, *_): pass
    def settimeout(self, _): pass
    def connect(self, address): self.address = address
    def send(self, message): self.message = message
    def recv(self, _): return struct.pack("!H", 0x101) + self.message[2:]


def page_headers(build):
    h = Message()
    h["Cross-Origin-Opener-Policy"] = "same-origin"
    h["Cross-Origin-Embedder-Policy"] = "require-corp"
    h["X-Content-Type-Options"] = "nosniff"
    for key, value in csp.page_headers(build["contentSecurityPolicy"]):
        h[key] = value
    return h


def header_block(text, path="/*"):
    """The headers of one path's block in a _headers file."""
    lines, block = text.splitlines(), None
    for line in lines:
        if not line.startswith((" ", "#")) and line.strip():
            block = {} if line.strip() == path else None
            if block is not None:
                found = block
        elif block is not None and line.startswith(" "):
            key, value = line.strip().split(": ", 1)
            block[key] = value
    return found


class LaunchTest(unittest.TestCase):
    def caches(self, root):
        dest = root / "4.0.2devnet"
        dest.mkdir()
        for name, source in (("block", "blockchain"), ("transaction", "transaction")):
            data = release.REPO / f"crates/ledger/src/proofs/data/devnet_{source}_verifier_index.json"
            (dest / f"{name}_verifier_index.postcard").write_bytes(hashlib.sha256(data.read_bytes()).digest() + b"fixture")
        return dest

    def test_cache_pair_and_source_before_build(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            dest = self.caches(root)
            release.verify_caches(root)
            for name in ("block", "transaction"):
                p = dest / f"{name}_verifier_index.postcard"
                good = p.read_bytes()
                for bad in (None, b"", good[:32], b"x" * len(good)):
                    with self.subTest(name=name, length=None if bad is None else len(bad)):
                        p.unlink()
                        if bad is not None:
                            p.write_bytes(bad)
                        with patch.object(release, "git", return_value="commit"), patch.object(release.subprocess, "run") as build:
                            with self.assertRaises(SystemExit):
                                release.verify_caches(root)
                            build.assert_not_called()
                        if not p.exists():
                            p.touch()
                p.write_bytes(good)

    def test_shared_ice_urls(self):
        cases = json.loads((HERE / "ice-url-cases.json").read_text())
        for group, expected in (("valid", True), ("invalid", False)):
            for url in cases[group]:
                with self.subTest(group=group, url=url):
                    self.assertEqual(check.valid_ice_url(url), expected)

    def test_credentials_expiry_and_shape(self):
        now = 2_000_000_000
        good = issuer.credentials(b"fixture only", "192.0.2.1", "198.51.100.7", now=now)
        check.check_credentials(good, now=now)
        for expiry in (now - 1, now, now + 60, now + 86461):
            bad = json.loads(json.dumps(good))
            bad[1]["username"] = f"{expiry}:fixture"
            with self.assertRaisesRegex(AssertionError, "expiry"):
                check.check_credentials(bad, now=now)
        for bad in ([], {}, [{"urls": []}], [{"urls": ["turn:example.org"], "username": "static", "credential": "fixture"}]):
            with self.assertRaises(AssertionError):
                check.check_credentials(bad, now=now)

    def test_credentials_one_username_per_address_and_window(self):
        secret, now = b"fixture only", 2_000_000_000
        def user(client, at=now):
            return issuer.credentials(secret, "192.0.2.1", client, now=at)[1]["username"]
        self.assertEqual(user("198.51.100.7"), user("198.51.100.7", now + 60))
        self.assertNotEqual(user("198.51.100.7"), user("198.51.100.8"))
        # IPv6 by its /64; an IPv4-mapped address is its IPv4 address.
        self.assertEqual(user("2001:db8:1:2::1"), user("2001:db8:1:2:ffff::9"))
        self.assertNotEqual(user("2001:db8:1:2::1"), user("2001:db8:1:3::1"))
        self.assertEqual(user("::ffff:198.51.100.7"), user("198.51.100.7"))
        # A new window, a new username with a later expiry.
        window = issuer.WINDOW
        later = user("198.51.100.7", now + window)
        self.assertNotEqual(later, user("198.51.100.7"))
        self.assertGreater(int(later.split(":")[0]), int(user("198.51.100.7").split(":")[0]))
        for at in (now, now // window * window, now // window * window + window - 1):
            expiry = int(user("198.51.100.7", at).split(":")[0])
            self.assertTrue(at + window < expiry <= at + 2 * window)
        self.assertNotIn("198", user("198.51.100.7"))
        with self.assertRaises(ValueError):
            user("not an address")

    def test_turn_guard_accepts_the_kit_config_only(self):
        text = (HERE / "turnserver.conf").read_text()
        text = text.replace("<VPS_PUBLIC_IPV4>", "192.0.2.10").replace("<TURN_AUTH_SECRET>", "a" * 64)
        relay = {"WEBRTC_UDP_PORTS": ["40000-40999"]}
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "turnserver.conf"
            path.write_text(text)
            rules = guard.rules(guard.settings(path), relay, 108)
            self.assertIn("ip daddr 192.0.2.10 udp sport 49152-49999 udp dport 40000-40999", rules)
            for extra in ("cli", "dtls", "rfc5780", "mobility", "no-auth", "user-quota=0",
                          "external-ip=192.0.2.11", "prometheus"):
                path.write_text(text + "\n" + extra + "\n")
                with self.subTest(extra=extra), self.assertRaises(ValueError):
                    guard.rules(guard.settings(path), relay, 108)
            path.write_text(text.replace("no-tcp-relay\n", ""))
            with self.assertRaises(ValueError):
                guard.rules(guard.settings(path), relay, 108)

    def test_assets_both_origin_modes_and_each_header(self):
        origin = "https://minaweb.example"
        build = {"wasm": {"path": "/node.wasm"}, "circuitsUrl": "/circuits"}
        calls = []
        def get(url, method, headers):
            calls.append((url, method, headers))
            h = Message()
            h["Content-Type"] = "application/wasm" if url.endswith(".wasm") else "application/octet-stream"
            h["Access-Control-Allow-Origin"] = origin
            h["Cross-Origin-Resource-Policy"] = "cross-origin"
            source = "blockchain" if "block_verifier" in url else "transaction"
            data = release.REPO / f"crates/ledger/src/proofs/data/devnet_{source}_verifier_index.json"
            return 200, h, hashlib.sha256(data.read_bytes()).digest() + b"fixture"
        with patch.object(check, "get", side_effect=get):
            check.check_assets(build, origin)
        self.assertEqual(len(calls), 6)
        self.assertEqual([c[2] for c in calls], [{}, {"Origin": origin}] * 3)
        for index in range(6):
            for broken in ("Content-Type", "Access-Control-Allow-Origin", "Cross-Origin-Resource-Policy", "body", "status"):
                n = 0
                def corrupt(url, method, headers):
                    nonlocal n
                    status, h, body = get(url, method, headers)
                    if n == index:
                        if broken == "status": status = 404
                        elif broken == "body": body = b"wrong"
                        else: del h[broken]
                    n += 1
                    return status, h, body
                if broken == "body" and index < 2: continue
                with self.subTest(index=index, broken=broken), patch.object(check, "get", side_effect=corrupt):
                    with self.assertRaises(AssertionError):
                        check.check_assets(build, origin)

    def test_policy_origins(self):
        self.assertEqual(csp.relay_origin(f"/ip4/192.0.2.1/tcp/3540/webrtc/http/p2p/{PEER}"), "http://192.0.2.1:3540")
        self.assertEqual(csp.relay_origin(f"/dns4/relay.minaweb.org/tcp/443/webrtc/https/p2p/{PEER}"), "https://relay.minaweb.org")
        self.assertEqual(csp.relay_origin(f"/dns/relay.example/tcp/8443/webrtc/https/p2p/{PEER}"), "https://relay.example:8443")
        for bad in (f"/ip6/2001:db8::1/tcp/80/webrtc/http/p2p/{PEER}", f"/ip4/192.0.2.1/tcp/3540/p2p/{PEER}", f"/ip4/999.0.2.1/tcp/1/webrtc/http/p2p/{PEER}", "/dns4/x/tcp/1/webrtc/ftp/p2p/x"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                csp.relay_origin(bad)
        directives = csp.parse(public_build()["contentSecurityPolicy"])
        self.assertEqual(directives["connect-src"], ["'self'", "https://assets.minaweb.org", "https://relay.minaweb.org"])
        local = csp.parse(csp.policy([f"/ip4/192.0.2.1/tcp/3540/webrtc/http/p2p/{PEER}"]))
        self.assertEqual(local["connect-src"], ["'self'", "http://192.0.2.1:3540"])
        self.assertEqual(local["script-src"], ["'self'", "'wasm-unsafe-eval'"])
        with self.assertRaises(ValueError):
            csp.parse("default-src 'none'; default-src *")

    def test_page_headers_exact(self):
        build = public_build()
        check.check_page_headers(page_headers(build), build, "https://relay.minaweb.org")
        policy = build["contentSecurityPolicy"]
        def served(name, value):
            h = page_headers(build)
            del h[name]
            if value is not None:
                h[name] = value
            return h
        breaks = {
            "no policy": (served("Content-Security-Policy", None), build),
            "inline scripts": (served("Content-Security-Policy", policy.replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")), build),
            "eval": (served("Content-Security-Policy", policy.replace("'wasm-unsafe-eval'", "'unsafe-eval'")), build),
            "framing": (served("Content-Security-Policy", policy.replace("frame-ancestors 'none'", "frame-ancestors 'self'")), build),
            "another connect host": (served("Content-Security-Policy", policy.replace("connect-src 'self'", "connect-src 'self' https://evil.example")), build),
            "no X-Frame-Options": (served("X-Frame-Options", None), build),
            "SAMEORIGIN": (served("X-Frame-Options", "SAMEORIGIN"), build),
            "Permissions-Policy": (served("Permissions-Policy", "camera=()"), build),
            "nosniff": (served("X-Content-Type-Options", None), build),
            "stale build-info": (page_headers(build), {**build, "contentSecurityPolicy": policy + "; img-src *"}),
            "another relay": (page_headers(build), {**build, "relay": build["relay"].replace("relay.minaweb.org", "relay2.minaweb.org")}),
        }
        report_only = served("Content-Security-Policy", None)
        report_only["Content-Security-Policy-Report-Only"] = policy
        breaks["report-only is not enforced"] = (report_only, build)
        twice = page_headers(build)
        twice["Content-Security-Policy"] = "default-src *"
        breaks["two policies"] = (twice, build)
        for name, (headers, info) in breaks.items():
            with self.subTest(name), self.assertRaises(AssertionError):
                check.check_page_headers(headers, info, "https://relay.minaweb.org")
        # Strict whatever the build says: a build-info and header that agree
        # on an unsafe source still fail.
        loose = {**build, "contentSecurityPolicy": policy.replace("style-src 'self'", "style-src 'self' 'unsafe-inline'")}
        with patch.object(check.csp, "policy", return_value=loose["contentSecurityPolicy"]), self.assertRaisesRegex(AssertionError, "style-src"):
            check.check_page_headers(page_headers(loose), loose, "https://relay.minaweb.org")

    def test_headers_files_carry_the_policy(self):
        build = public_build()
        with tempfile.TemporaryDirectory() as temp:
            info = Path(temp) / "build-info.json"
            info.write_text(json.dumps(build))
            for template in (HERE / "_headers", HERE.parent / "site/_headers"):
                with self.subTest(template=template.relative_to(HERE.parents[1])):
                    block = header_block(csp.headers_file(template, info))
                    self.assertEqual(block["Content-Security-Policy"], build["contentSecurityPolicy"])
                    self.assertEqual(block["X-Frame-Options"], csp.X_FRAME_OPTIONS)
                    self.assertEqual(block["Permissions-Policy"], csp.PERMISSIONS_POLICY)
                    self.assertEqual(block["X-Content-Type-Options"], "nosniff")
            info.write_text(json.dumps({**build, "contentSecurityPolicy": None}))
            with self.assertRaises(ValueError):
                csp.headers_file(HERE / "_headers", info)
        for caddy in (HERE / "local.Caddyfile", HERE.parent / "site/site.caddy"):
            with self.subTest(caddy=caddy.name):
                text = caddy.read_text()
                self.assertIn(f'Permissions-Policy "{csp.PERMISSIONS_POLICY}"', text)
                self.assertIn(f"X-Frame-Options {csp.X_FRAME_OPTIONS}", text)
                self.assertRegex(text, r'Content-Security-Policy "\{\$[A-Z_]+_CSP:default-src \'none\'\}"')
        serve = json.loads(csp.serve_json(HERE.parents[1] / "regression/webnode/serve.json", build["contentSecurityPolicy"]))
        every = {h["key"]: h["value"] for h in serve["headers"][0]["headers"]}
        self.assertEqual(every["Content-Security-Policy"], build["contentSecurityPolicy"])
        self.assertEqual(every["X-Frame-Options"], csp.X_FRAME_OPTIONS)

    def test_automated_cli_never_claims_browser_pass(self):
        override = None
        build = public_build()
        def get(url, method="GET", headers=None):
            h = page_headers(build)
            h["Cache-Control"] = "no-store"
            if url.endswith("build-info.json"):
                return 200, h, json.dumps(build).encode()
            if url.endswith("ice-servers.json"):
                ice = issuer.credentials(b"test only", "192.0.2.1", "198.51.100.7")
                if override is not None:
                    ice[1]["urls"] = override
                h["Access-Control-Allow-Origin"] = "https://minaweb.org"
                return 200, h, json.dumps(ice).encode()
            if url == "http://native/status":
                return 200, h, json.dumps(NATIVE).encode()
            if url.endswith(("/status", "/graphql", "/state", "/metrics")): return 404, h, b""
            if url.endswith("/signal/x"): return 400, h, b""
            return 200, h, b""
        cases = json.loads((HERE / "ice-url-cases.json").read_text())
        overrides = [None] + [[url] for url in cases["invalid"]] + [["turn:example.org", "https://example.org"]]
        for override in overrides:
            with self.subTest(urls=override), tempfile.TemporaryDirectory() as temp:
                out = Path(temp) / "gate"
                with patch.object(sys, "argv", ["check.py", str(out), "--relay", "https://relay.minaweb.org", "--turn-ip", "192.0.2.1", "--automated-only"]), patch.dict(os.environ, RELAY_HTTP="http://native"), patch.object(check, "get", side_effect=get), patch.object(check, "check_assets"), patch.object(check.socket, "socket", return_value=Stun()), patch.object(check.subprocess, "run") as browser:
                    if override is None:
                        check.main()
                    else:
                        with self.assertRaisesRegex(AssertionError, "invalid ICE URLs"):
                            check.main()
                    browser.assert_not_called()
                self.assertEqual((out / "AUTOMATED-PASS").exists(), override is None)
                self.assertFalse((out / "PASS").exists())


    def two_relay_get(self, build, ice_ip=None, cors="https://minaweb.org"):
        """Each relay's routes, its credential service with its own TURN
        address (ice_ip overrides relay2's), and its native node."""
        ips = {"relay1": "192.0.2.1", "relay2": ice_ip or "198.51.100.2"}
        def get(url, method="GET", headers=None):
            h = page_headers(build)
            h["Cache-Control"] = "no-store"
            if url.endswith("build-info.json"):
                return 200, h, json.dumps(build).encode()
            if url.endswith("/ice-servers.json"):
                name = url.split("//")[1].split(".")[0]
                h["Access-Control-Allow-Origin"] = cors
                return 200, h, json.dumps(issuer.credentials(b"test only", ips[name], "198.51.100.7")).encode()
            if url.startswith("http://native"):
                return 200, h, json.dumps(NATIVE).encode()
            if url.endswith(("/status", "/graphql", "/state", "/metrics")): return 404, h, b""
            if url.endswith("/signal/x"): return 400, h, b""
            return 200, h, b""
        return get

    def run_two(self, build, get, relays=("relay1", "relay2"), natives=2):
        argv = ["check.py", "OUT", "--automated-only"]
        for name, ip in zip(relays, ("192.0.2.1", "198.51.100.2")):
            argv += ["--relay", f"https://{name}.minaweb.org", "--turn-ip", ip]
        argv += [a for n in range(natives) for a in ("--relay-http", f"http://native{n}")]
        with tempfile.TemporaryDirectory() as temp:
            argv[1] = str(Path(temp) / "gate")
            stun = []
            def socket(*_):
                stun.append(Stun())
                return stun[-1]
            with patch.object(sys, "argv", argv), patch.object(check, "get", side_effect=get), \
                    patch.object(check, "check_assets"), patch.object(check.socket, "socket", side_effect=socket), \
                    patch.object(check.subprocess, "run") as browser:
                check.main()
                browser.assert_not_called()
            self.assertEqual([s.address for s in stun], [("192.0.2.1", 3478), ("198.51.100.2", 3478)])
            preflight = json.loads((Path(argv[1]) / "preflight.json").read_text())
            self.assertEqual(len(preflight["relays"]), 2)
            return (Path(argv[1]) / "AUTOMATED-PASS").read_text()

    def test_two_relays_each_checked(self):
        build = two_relay_build()
        self.assertEqual(csp.parse(build["contentSecurityPolicy"])["connect-src"],
                         ["'self'", "https://assets.minaweb.org", "https://relay1.minaweb.org",
                          "https://relay2.minaweb.org"])
        self.assertIn("2 relays", self.run_two(build, self.two_relay_get(build)))
        # relay2's issuer handing out relay1's TURN, an issuer without CORS for
        # the site, a build that dials only one of them, and a relay without
        # its operator API all fail.
        with self.assertRaisesRegex(AssertionError, "this relay's address"):
            self.run_two(build, self.two_relay_get(build, ice_ip="192.0.2.1"))
        with self.assertRaisesRegex(AssertionError, "credential CORS"):
            self.run_two(build, self.two_relay_get(build, cors="*"))
        one = {**build, "relays": build["relays"][:1]}
        with self.assertRaisesRegex(AssertionError, "relays are not the ones given"):
            self.run_two(one, self.two_relay_get(one))
        with self.assertRaises(SystemExit):
            self.run_two(build, self.two_relay_get(build), natives=1)
        # A page whose ICE lists miss a relay's TURN.
        partial = {**build, "iceServers": build["iceServers"][:1]}
        with self.assertRaisesRegex(AssertionError, "each relay's TURN"):
            self.run_two(partial, self.two_relay_get(partial))

    def test_browser_run_needs_every_relay(self):
        ips = ["192.0.2.1", "198.51.100.2"]
        def line(peers, pairs, sync="Synced", tip=500):
            status = {"best_tip": tip, "sync": sync, "peers": peers}
            rtc = {"open": len(pairs), "rx_mb": 1, "tx_mb": 0, "pairs": pairs}
            return f"60s rss=1MB wasm=[1]MB status={json.dumps(status)} rtc={json.dumps(rtc)}"
        head = "crossOriginIsolated=true\n"
        direct = ["host>host:192.0.2.1:40001", "srflx>host:198.51.100.2:40002"]
        turn = ["relay>host:192.0.2.1:40001", "relay>host:198.51.100.2:40002"]
        check.check_browser_run(head + line(2, direct), "direct", ips, 500)
        check.check_browser_run(head + line(1, direct[:1]) + "\n" + line(2, turn, tip=499), "turn", ips, 500)
        for mode, log in {
            "direct": head + line(1, direct[:1]),
            "turn": head + line(2, direct),
            "turn ": head + line(2, turn, sync="Bootstrap"),
            " direct": head + line(2, direct, tip=497),
            "direct  ": line(2, direct),
        }.items():
            with self.subTest(mode=mode), self.assertRaises(AssertionError):
                check.check_browser_run(log, mode.strip(), ips, 500)

    def test_public_relays_setting(self):
        self.assertEqual(release.public_relays({"RELAYS": f"relay1.minaweb.org={PEER}  relay2.minaweb.org={PEER2}"}),
                         [("relay1.minaweb.org", PEER), ("relay2.minaweb.org", PEER2)])
        # One relay, as before.
        self.assertEqual(release.public_relays({"RELAY_PEER_ID": PEER, "RELAY_DOMAIN": "relay1.minaweb.org"}),
                         [("relay1.minaweb.org", PEER)])
        for bad in ({}, {"RELAY_PEER_ID": PEER}, {"RELAY_DOMAIN": "relay1.minaweb.org"}, {"RELAYS": "relay1.minaweb.org"}, {"RELAYS": f"relay1.minaweb.org={PEER} relay1.minaweb.org={PEER2}"},
                    {"RELAYS": f"relay1.minaweb.org={PEER} relay2.minaweb.org={PEER}"},
                    {"RELAYS": f"relay1.minaweb.org={PEER}", "RELAY_PEER_ID": PEER},
                    {"RELAYS": f"https://relay1.minaweb.org={PEER}"}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                release.public_relays(bad)

    def test_release_builds_every_relay_with_its_own_credentials(self):
        seen = {}
        def run(command, env, **_):
            seen.update(env)
            raise SystemExit("stop after the build's settings")
        environ = {"RELAYS": f"relay1.minaweb.org={PEER} relay2.minaweb.org={PEER2}",
                   "PROVER_CIRCUITS": "/provers", "PATH": os.environ["PATH"]}
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, environ, clear=True), \
                patch.object(sys, "argv", ["release.py", "HEAD", str(Path(temp) / "out")]), \
                patch.object(release, "git", side_effect=lambda *a: "" if a[0] == "status" else "c" * 40), \
                patch.object(release, "verify_caches"), patch.object(release, "build_pins"), \
                patch.object(release.subprocess, "run", side_effect=run), self.assertRaises(SystemExit):
            release.main()
        self.assertEqual(seen["RELAY"], f"/dns4/relay1.minaweb.org/tcp/443/webrtc/https/p2p/{PEER},"
                                        f"/dns4/relay2.minaweb.org/tcp/443/webrtc/https/p2p/{PEER2}")
        self.assertEqual(seen["ICE_SERVERS_URLS"], "https://relay1.minaweb.org/ice-servers.json,"
                                                   "https://relay2.minaweb.org/ice-servers.json")
        self.assertEqual(seen["ICE_SERVERS"], "")
        self.assertNotIn("RELAYS", seen)

    def test_relay_list(self):
        one = f"/dns4/relay1.minaweb.org/tcp/443/webrtc/https/p2p/{PEER}"
        two = f"/ip4/192.0.2.1/tcp/3540/webrtc/http/p2p/{PEER2}"
        for text in (f"{one},{two}", f" {one} ,\n{two}\n", f"{one} {two}"):
            with self.subTest(text=text):
                self.assertEqual(csp.relay_list(text), [one, two])
        self.assertEqual(csp.relay_list(one), [one])
        for bad in ("", " , ", f"{one},{one}", f"{one},{two.replace(PEER2, PEER)}",
                    f"{one},/ip6/2001:db8::1/tcp/80/webrtc/http/p2p/{PEER2}",
                    f"{one},/ip4/192.0.2.1/tcp/3540/p2p/{PEER2}"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                csp.relay_list(bad)

    def test_cloud_init_masks_coturn_and_updates(self):
        import yaml
        text = (HERE / "cloud-init.yaml").read_text()
        self.assertTrue(text.startswith("#cloud-config\n"))
        config = yaml.safe_load(text)
        # bootcmd runs before packages are installed, and only once, so the
        # runbook's later unmask survives a reboot.
        self.assertIn(["cloud-init-per", "once", "mask-coturn", "systemctl", "mask", "coturn.service"],
                      config["bootcmd"])
        # Ubuntu's coturn 4.6.1 (CVE-2026-68554) is never installed.
        self.assertNotIn("coturn", config["packages"])
        self.assertIn("libevent-openssl-2.1-7t64", config["packages"])
        self.assertTrue(config["package_upgrade"] and config["package_reboot_if_required"])
        # The apt settings are the kit file's, comments aside.
        def settings(text):
            return [l.strip() for l in text.splitlines() if l.strip() and not l.strip().startswith("//")]
        written = {f["path"]: f["content"] for f in config["write_files"]}
        self.assertEqual(settings(written["/etc/apt/apt.conf.d/52mina-relay"]),
                         settings((HERE / "unattended-upgrades-relay.conf").read_text()))
        self.assertIn(["systemctl", "disable", "--now", "nftables.service"], config["runcmd"])
        # No host name, address or secret: the same file serves every relay.
        self.assertNotRegex(text, r"minaweb\.org|[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+|secret:|password")

    def test_relay_only_before_the_page(self):
        build = two_relay_build()
        get = self.two_relay_get(build)
        fetched = []
        def relay_get(url, method="GET", headers=None):
            fetched.append(url)
            return get(url, method, headers)
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "gate"
            argv = ["check.py", str(out), "--relay-only",
                    "--relay", "https://relay1.minaweb.org", "--turn-ip", "192.0.2.1", "--relay-http", "http://native1",
                    "--relay", "https://relay2.minaweb.org", "--turn-ip", "198.51.100.2", "--relay-http", "http://native2"]
            with patch.object(sys, "argv", argv), patch.object(check, "get", side_effect=relay_get), \
                    patch.object(check.socket, "socket", side_effect=lambda *_: Stun()), \
                    patch.object(check.subprocess, "run") as browser:
                check.main()
                browser.assert_not_called()
            self.assertIn("2 relays", (out / "RELAY-PASS").read_text())
            self.assertFalse((out / "PASS").exists() or (out / "AUTOMATED-PASS").exists())
            self.assertEqual(len(json.loads((out / "relays.json").read_text())), 2)
        # The site is never fetched, and each relay's own credential service is.
        self.assertFalse(any(u.startswith("https://minaweb.org") for u in fetched))
        self.assertIn("https://relay2.minaweb.org/ice-servers.json", fetched)
        # A relay handing out another relay's TURN fails here too.
        with tempfile.TemporaryDirectory() as temp, self.assertRaisesRegex(AssertionError, "this relay's address"):
            argv[1] = str(Path(temp) / "gate")
            with patch.object(sys, "argv", argv), \
                    patch.object(check, "get", side_effect=self.two_relay_get(build, ice_ip="192.0.2.1")), \
                    patch.object(check.socket, "socket", side_effect=lambda *_: Stun()):
                check.main()
        # No relay, no default.
        with patch.object(sys, "argv", ["check.py", "x", "--turn-ip", "192.0.2.1"]), \
                patch("sys.stderr"), self.assertRaises(SystemExit):
            check.main()


def brotli(data):
    """The bytes brotli-compressed, as build.sh makes the WASM's .br (node)."""
    import subprocess
    script = ('const z = require("zlib"); const c = [];'
              'process.stdin.on("data", d => c.push(d));'
              'process.stdin.on("end", () => process.stdout.write(z.brotliCompressSync(Buffer.concat(c))));')
    return subprocess.run(["node", "-e", script], input=data, check=True, capture_output=True).stdout


class BrotliWasmTest(unittest.TestCase):
    WASM = b"\0asm" + bytes(range(256)) * 64

    def release_dir(self, temp, encoded=None):
        out = Path(temp)
        (out / "upload/wasm").mkdir(parents=True)
        name = "mina_node_web_bg.0123456789abcdef.wasm"
        (out / "upload/wasm" / name).write_bytes(self.WASM)
        (out / "upload/wasm" / (name + ".br")).write_bytes(brotli(self.WASM) if encoded is None else encoded)
        return out, name

    def test_the_brotli_file_goes_up_under_the_wasm_name(self):
        pinned = hashlib.sha256(self.WASM).hexdigest()
        with tempfile.TemporaryDirectory() as temp:
            out, name = self.release_dir(temp)
            target = release.stage_brotli_wasm(out, pinned)
            self.assertEqual(target, out / "upload-br/wasm" / name)
            self.assertEqual(release.brotli_sha256(target), pinned)
            # The raw WASM is kept, out of both uploads.
            self.assertEqual((out / "wasm" / name).read_bytes(), self.WASM)
            self.assertEqual(sorted(p.name for p in (out / "upload/wasm").iterdir()), [])

    def test_a_brotli_file_of_other_bytes_is_refused(self):
        pinned = hashlib.sha256(self.WASM).hexdigest()
        with tempfile.TemporaryDirectory() as temp:
            out, _ = self.release_dir(temp, encoded=brotli(self.WASM[:-1]))
            with self.assertRaisesRegex(SystemExit, "does not decode to the pinned"):
                release.stage_brotli_wasm(out, pinned)

    def test_deploy_without_the_option_is_unchanged(self):
        commands = release.deploy_commands("minaweb-assets", "releases/c/r", "minaweb", "c" * 40)
        # Line for line the deploy.sh that release.py wrote before the option.
        self.assertEqual(commands, [
            "#!/usr/bin/env bash", "set -euo pipefail",
            'cd -- "$(dirname -- "${BASH_SOURCE[0]}")"', "sha256sum -c SHA256SUMS",
            "# Configure the private rclone remote 'r2' before running.",
            "rclone copy upload r2:minaweb-assets/releases/c/r --immutable --checksum "
            "--header-upload 'Cache-Control: public, max-age=31536000, immutable'",
            "rclone check upload r2:minaweb-assets/releases/c/r --download --one-way",
            "# Verify asset CORS/CORP and MIME types before deploying Pages.",
            'test "${ASSET_HEADERS_VERIFIED:-}" = yes',
            "npx wrangler pages deploy browser --project-name minaweb --branch main "
            f"--commit-hash {'c' * 40} --commit-dirty=false"])

    def test_deploy_with_brotli_uploads_checks_then_deploys(self):
        wasm = {"url": "https://assets.minaweb.org/releases/c/r/wasm/x.wasm", "sha256": "a" * 64}
        commands = release.deploy_commands("minaweb-assets", "releases/c/r", "minaweb", "c" * 40, wasm)
        upload = next(c for c in commands if c.startswith("rclone copy upload-br "))
        self.assertIn("--header-upload 'Content-Encoding: br'", upload)
        self.assertIn("--header-upload 'Content-Type: application/wasm'", upload)
        self.assertIn("--immutable --checksum", upload)
        order = [next(i for i, c in enumerate(commands) if c.startswith(p)) for p in (
            "rclone copy upload ", "rclone copy upload-br ", "rclone check upload ",
            "rclone check upload-br ", "python3 -B check-wasm-encoding.py ", "npx wrangler")]
        self.assertEqual(order, sorted(order))

    def test_the_encoding_check_reads_each_answer_raw(self):
        pinned = hashlib.sha256(self.WASM).hexdigest()
        encoded = brotli(self.WASM)
        def headers(encoding):
            message = Message()
            message["Content-Type"] = "application/wasm"
            message["Access-Control-Allow-Origin"] = "https://minaweb.org"
            if encoding:
                message["Content-Encoding"] = encoding
            return message
        def edge(url, accept):
            if accept and "br" in accept:
                return 200, headers("br"), encoded
            return 200, headers(None), self.WASM
        lines = encoding_check.check("https://x/w.wasm", pinned, fetch=edge)
        self.assertEqual(len(lines), len(encoding_check.CLIENTS))
        failures = {
            # Brotli to a client that asked for gzip only.
            "not one of": lambda url, accept: (200, headers("br"), encoded),
            # A browser given the edge's own zstd instead of the brotli file.
            "Content-Encoding zstd": lambda url, accept: (200, headers("zstd"), b""),
            # Brotli bytes labelled as plain: decoding by the label fails the hash.
            "not the pinned": lambda url, accept: (200, headers("br" if accept and "br" in accept else None), encoded),
        }
        for message, fetch in failures.items():
            with self.subTest(message=message), self.assertRaisesRegex(AssertionError, message):
                encoding_check.check("https://x/w.wasm", pinned, fetch=fetch)


if __name__ == "__main__":
    unittest.main()
