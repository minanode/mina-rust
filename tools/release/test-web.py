#!/usr/bin/env python3
"""Exercise the downloadable server's integrity and HTTP boundaries."""
import argparse
import gzip
import hashlib
import os
import subprocess
import sys
import tarfile
from unittest.mock import patch
import http.client
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
from http.server import ThreadingHTTPServer

spec = importlib.util.spec_from_file_location("serve_web", Path(__file__).with_name("serve-web.py"))
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)
RELAY = "/ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ"
spec = importlib.util.spec_from_file_location(
    "csp", Path(__file__).resolve().parents[1] / "regression/webnode/csp.py")
csp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(csp)


class BundleTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        payload = {"browser/index.html": b"<html>node</html>",
                   "browser/node.wasm": b"\0asm\x01\0\0\0", "browser/og.png": b"\x89PNG\r\n\x1a\n",
                   "README.md": b"read me"}
        files = []
        for name, data in payload.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            files.append({"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        self.manifest = {"schema": 1, "network": "devnet", "relay": RELAY, "files": files,
                         "build": {"contentSecurityPolicy": csp.policy([RELAY])}}
        self.save()

    def save(self):
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        (self.root / "SHA256SUMS").write_text("".join(f"{f['sha256']}  {f['path']}\n" for f in self.manifest['files'])
                                             + f"{web.sha(self.root / 'manifest.json')}  manifest.json\n")

    def test_valid_bundle(self):
        self.assertEqual(web.verify(self.root), self.manifest)

    def test_corrupt_same_size_asset(self):
        (self.root / "browser/node.wasm").write_bytes(b"badbytes")
        with self.assertRaisesRegex(ValueError, "integrity failure"):
            web.verify(self.root)

    def test_missing_and_extra_files(self):
        (self.root / "browser/extra.js").write_text("unlisted")
        with self.assertRaisesRegex(ValueError, "unlisted"):
            web.verify(self.root)
        (self.root / "browser/extra.js").unlink()
        (self.root / "browser/node.wasm").unlink()
        with self.assertRaisesRegex(ValueError, "integrity failure"):
            web.verify(self.root)

    def test_symlink_refused(self):
        (self.root / "browser/linked").symlink_to(self.root / "README.md")
        self.manifest["files"].append({"path": "browser/linked", "bytes": 7, "sha256": web.sha(self.root / "README.md")})
        self.save()
        with self.assertRaisesRegex(ValueError, "integrity failure"):
            web.verify(self.root)

    def test_checksum_manifest_mismatch(self):
        self.manifest["relay"] = RELAY.replace("8303", "8304")
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
            web.verify(self.root)

    def test_unsafe_manifest_path(self):
        self.manifest["files"][0]["path"] = "../outside"
        self.save()
        with self.assertRaisesRegex(ValueError, "unsafe"):
            web.verify(self.root)

    def test_http_isolation_and_override(self):
        override = RELAY.replace("8303", "8304")
        server = ThreadingHTTPServer(("127.0.0.1", 0), http.client.HTTPConnection)
        port = server.server_port
        server.RequestHandlerClass = web.handler(self.root, web.verify(self.root), override, port)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            def get(path, headers=None):
                conn = http.client.HTTPConnection("127.0.0.1", port)
                conn.request("GET", path, headers=headers or {})
                response = conn.getresponse()
                result = response.status, dict(response.getheaders()), response.read()
                conn.close()
                return result
            status, headers, body = get("/local-relay.json")
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(body), {"relays": [override]})
            self.assertEqual(headers["Cross-Origin-Opener-Policy"], "same-origin")
            self.assertEqual(headers["Cross-Origin-Embedder-Policy"], "require-corp")
            # The build's policy, naming the relay in use instead of its own.
            for path in ("/", "/key", "/local-relay.json"):
                page = get(path)[1]
                self.assertEqual(page["Content-Security-Policy"], csp.policy([override]))
                self.assertEqual(page["X-Frame-Options"], csp.X_FRAME_OPTIONS)
                self.assertEqual(page["Permissions-Policy"], csp.PERMISSIONS_POLICY)
            self.assertEqual(get("/node.wasm")[1]["Content-Type"], "application/wasm")
            self.assertEqual(get("/og.png")[1]["Content-Type"], "image/png")
            for route in ("/blocks/test", "/accounts/test", "/key", "/send",
                          "/snark-worker", "/block-producer"):
                self.assertEqual(get(route)[2], b"<html>node</html>")
            for path in ("/README.md", "/../README.md", "/missing.js", "/browser/"):
                self.assertEqual(get(path)[0], 404)
            self.assertEqual(get("/", {"Host": "untrusted.example"})[0], 403)
            self.assertEqual(get("/", {"Origin": "https://untrusted.example"})[0], 403)
            self.assertEqual(web.verify(self.root), self.manifest)
        finally:
            server.shutdown()
            thread.join()
            server.server_close()


    def test_policy_follows_the_relay(self):
        cases = json.loads(Path(__file__).with_name("relay-cases.json").read_text())
        for relay in cases["valid"]:
            with self.subTest(relay=relay):
                if relay.startswith("/ip6/"):
                    with self.assertRaises(ValueError):
                        web.page_policy(self.manifest, relay)
                else:
                    self.assertEqual(web.page_policy(self.manifest, relay), csp.policy([relay]))
        for broken in ({}, {"contentSecurityPolicy": None},
                       {"contentSecurityPolicy": csp.policy([RELAY.replace("8303", "9")])}):
            with self.subTest(build=broken), self.assertRaises(ValueError):
                web.page_policy({**self.manifest, "build": broken}, RELAY)

    def test_two_relays(self):
        second = "/dns4/relay2.example.org/tcp/443/webrtc/https/p2p/12D3KooWRhfZ3wQKx4LWJp4xc45UYoT6UqASoLKgpx1bE52MjSLd"
        built = {**self.manifest, "relays": [RELAY, second],
                 "build": {"contentSecurityPolicy": csp.policy([RELAY, second], "https://assets.example.org/w")}}
        del built["relay"]
        self.assertEqual(web.bundle_relays(built), [RELAY, second])
        # Its own relays, others in their place, or one: always the policy
        # the build would have made for them.
        for relays in ([RELAY, second], [second], [RELAY.replace("8303", "8304"), RELAY.replace("8303", "8305")]):
            with self.subTest(relays=relays):
                self.assertEqual(web.page_policy(built, relays),
                                 csp.policy(relays, "https://assets.example.org/w"))
        self.assertEqual(web.page_policy(self.manifest, [RELAY, second]), csp.policy([RELAY, second]))
        broken = {**built, "build": {"contentSecurityPolicy": csp.policy([RELAY])}}
        with self.assertRaisesRegex(ValueError, "does not name its relays"):
            web.page_policy(broken, [RELAY])
        overrides = [RELAY.replace("8303", "8304"), RELAY.replace("8303", "8305")]
        server = ThreadingHTTPServer(("127.0.0.1", 0), http.client.HTTPConnection)
        port = server.server_port
        server.RequestHandlerClass = web.handler(self.root, web.verify(self.root), overrides, port)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            conn = http.client.HTTPConnection("127.0.0.1", port)
            conn.request("GET", "/local-relay.json")
            response = conn.getresponse()
            self.assertEqual(json.loads(response.read()), {"relays": overrides})
            self.assertEqual(response.getheader("Content-Security-Policy"), csp.policy(overrides))
            conn.close()
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

    def test_cli_takes_several_relays(self):
        script = str(Path(__file__).with_name("serve-web.py"))
        copy = self.root / "serve-web.py"
        copy.write_text(Path(script).read_text())
        self.manifest["files"].append({"path": "serve-web.py", "bytes": copy.stat().st_size, "sha256": web.sha(copy)})
        self.save()
        two = ["--relay", RELAY, "--relay", RELAY.replace("8303", "8304")]
        result = subprocess.run([sys.executable, "-B", str(copy), *two, "--verify-only"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run([sys.executable, "-B", str(copy), "--relay", RELAY, "--relay", RELAY,
                                 "--port", "1"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("given twice", result.stderr)


class ReleaseTest(unittest.TestCase):
    def test_bindgen_rejects_corrupt_cached_crate(self):
        spec = importlib.util.spec_from_file_location("bindgen", Path(__file__).with_name("deterministic-bindgen.py"))
        bindgen = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bindgen)
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / "registry/cache/test"
            cache.mkdir(parents=True)
            (cache / f"wasm-bindgen-cli-{bindgen.VERSION}.crate").write_bytes(b"corrupt")
            with patch.dict(os.environ, CARGO_HOME=temp):
                with self.assertRaisesRegex(ValueError, "pinned crate checksum mismatch"):
                    bindgen.crate_bytes("wasm-bindgen-cli")

    def test_relay_contract_and_runtime_cli(self):
        cases = json.loads(Path(__file__).with_name("relay-cases.json").read_text())
        for relay in cases["valid"]:
            self.assertEqual(web.relay_address(relay), relay)
        for relay in cases["invalid"]:
            with self.subTest(relay=relay):
                with self.assertRaises(argparse.ArgumentTypeError):
                    web.relay_address(relay)
                result = subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("serve-web.py")), "--relay", relay, "--verify-only"], capture_output=True, text=True)
                self.assertEqual(result.returncode, 2)
                self.assertIn("relay", result.stderr)
                # Packaging rejects bad RELAY before a build or clean-tree check.
                with patch.dict(os.environ, RELAY=relay):
                    result = subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("build-web.py")), __import__("tomllib").loads((Path(__file__).resolve().parents[2] / "Cargo.toml").read_text())["workspace"]["package"]["version"]], capture_output=True, text=True)
                self.assertEqual(result.returncode, 2)
                self.assertIn("set RELAY", result.stderr)

    def test_archive_metadata_and_environment(self):
        spec = importlib.util.spec_from_file_location("build_web", Path(__file__).with_name("build-web.py"))
        build = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(build)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / "mina-web-1.0.0"
            bundle.mkdir()
            for name in ("z.txt", "a.txt", "long" * 30 + ".js"):
                (bundle / name).write_text(name)
            first, second = root / "first.gz", root / "second.gz"
            build.write_archive(bundle, first, 1234567890)
            for file in bundle.iterdir():
                os.utime(file, (2000000000, 2000000000))
                file.chmod(0o777)
            build.write_archive(bundle, second, 1234567890)
            self.assertEqual(web.sha(first), web.sha(second))
            self.assertEqual(first.read_bytes()[3:8], b"\0" * 5)  # no name or wall-clock gzip header
            with tarfile.open(first) as tar:
                self.assertEqual(tar.getnames(), sorted(tar.getnames()))
                for member in tar:
                    self.assertEqual((member.uid, member.gid, member.uname, member.gname, member.mtime, member.mode), (0, 0, "", "", 1234567890, 0o644))
            with patch.dict(os.environ, GENESIS="unrecorded", MINAWEB_WASM_URL="unrecorded", RUSTFLAGS="unrecorded", VERGEN_BUILD_TIMESTAMP="unrecorded", WASM_BINDGEN="unrecorded"):
                env = build.build_environment(RELAY, root, 1234567890, "commit")
            self.assertNotIn("unrecorded", env.values())
            self.assertEqual(env["VERIFIER_CACHES"], "omit")
            self.assertEqual(env["SOURCE_DATE_EPOCH"], "1234567890")


if __name__ == "__main__":
    unittest.main()
