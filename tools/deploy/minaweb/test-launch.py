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
        good = issuer.credentials(b"fixture only", "192.0.2.1", now=now)
        check.check_credentials(good, now=now)
        for expiry in (now - 1, now, now + 60, now + 86461):
            bad = json.loads(json.dumps(good))
            bad[1]["username"] = f"{expiry}:fixture"
            with self.assertRaisesRegex(AssertionError, "expiry"):
                check.check_credentials(bad, now=now)
        for bad in ([], {}, [{"urls": []}], [{"urls": ["turn:example.org"], "username": "static", "credential": "fixture"}]):
            with self.assertRaises(AssertionError):
                check.check_credentials(bad, now=now)

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

    def test_automated_cli_never_claims_browser_pass(self):
        class Stun:
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def settimeout(self, _): pass
            def connect(self, _): pass
            def send(self, message): self.message = message
            def recv(self, _): return struct.pack("!H", 0x101) + self.message[2:]
        override = None
        def get(url, method="GET", headers=None):
            h = Message()
            h["Cross-Origin-Opener-Policy"] = "same-origin"
            h["Cross-Origin-Embedder-Policy"] = "require-corp"
            h["Cache-Control"] = "no-store"
            if url.endswith("build-info.json"):
                return 200, h, json.dumps({"network": "devnet", "wasm": {"pinnedInPage": True}}).encode()
            if url.endswith("ice-servers.json"):
                ice = issuer.credentials(b"test only", "192.0.2.1")
                if override is not None:
                    ice[1]["urls"] = override
                return 200, h, json.dumps(ice).encode()
            if url == "http://native/status":
                return 200, h, json.dumps({"network": "devnet", "peers": [{"connection_status": "Connected", "is_libp2p": True}], "transition_frontier": {"sync": {"status": "Synced"}}}).encode()
            if url.endswith(("/status", "/graphql", "/state", "/metrics")): return 404, h, b""
            if url.endswith("/signal/x"): return 400, h, b""
            return 200, h, b""
        cases = json.loads((HERE / "ice-url-cases.json").read_text())
        overrides = [None] + [[url] for url in cases["invalid"]] + [["turn:example.org", "https://example.org"]]
        for override in overrides:
            with self.subTest(urls=override), tempfile.TemporaryDirectory() as temp:
                out = Path(temp) / "gate"
                with patch.object(sys, "argv", ["check.py", str(out), "--turn-ip", "192.0.2.1", "--automated-only"]), patch.dict(os.environ, RELAY_HTTP="http://native"), patch.object(check, "get", side_effect=get), patch.object(check, "check_assets"), patch.object(check.socket, "socket", return_value=Stun()), patch.object(check.subprocess, "run") as browser:
                    if override is None:
                        check.main()
                    else:
                        with self.assertRaisesRegex(AssertionError, "invalid ICE URLs"):
                            check.main()
                    browser.assert_not_called()
                self.assertEqual((out / "AUTOMATED-PASS").exists(), override is None)
                self.assertFalse((out / "PASS").exists())


if __name__ == "__main__":
    unittest.main()
