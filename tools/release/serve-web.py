#!/usr/bin/env python3
"""Verify and serve an unpacked Mina web bundle on loopback (Python 3.11+)."""
import argparse
import hashlib
import ipaddress
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path, PurePosixPath
import re
from urllib.parse import unquote, urlsplit

RELAY = re.compile(r"/(ip4|ip6|dns4|dns6|dns)/([^/]+)/tcp/([0-9]+)/webrtc/(https?)/p2p/([^/]+)")
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def relay_address(value):
    match = RELAY.fullmatch(value)
    if not match:
        raise argparse.ArgumentTypeError("relay must use /HOST/address/tcp/port/webrtc/http(s)/p2p/peer-id")
    family, host, port, _, peer = match.groups()
    if len(port) > 5 or not 1 <= int(port) <= 65535:
        raise argparse.ArgumentTypeError("relay TCP port must be between 1 and 65535")
    try:
        if family == "ip4":
            ipaddress.IPv4Address(host)
        elif family == "ip6":
            if "%" in host:
                raise ValueError("scoped IPv6 is not supported")
            ipaddress.IPv6Address(host)
        elif (len(host.removesuffix(".")) > 253 or not all(
                re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?", label)
                for label in host.removesuffix(".").split("."))):
            raise ValueError("invalid DNS name")
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"relay has an invalid {family} address") from error
    # libp2p's inline Ed25519 public key: identity multihash, protobuf key.
    try:
        if len(peer) != 52:
            raise ValueError("length")
        number = 0
        for char in peer:
            number = number * 58 + BASE58.index(char)
        decoded = b"\0" * (len(peer) - len(peer.lstrip("1")))
        decoded += number.to_bytes((number.bit_length() + 7) // 8, "big")
        if len(decoded) != 38 or decoded[:6] != bytes.fromhex("002408011220"):
            raise ValueError("encoding")
    except ValueError as error:
        raise argparse.ArgumentTypeError("relay peer ID must encode an Ed25519 libp2p public key") from error
    return value


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify(root):
    manifest = json.loads((root / "manifest.json").read_text())
    if manifest["schema"] != 1 or manifest["network"] != "devnet":
        raise ValueError("unsupported bundle manifest")
    listed = set()
    for entry in manifest["files"]:
        name = entry["path"]
        parts = PurePosixPath(name)
        if (not name or parts.is_absolute() or ".." in parts.parts
                or str(parts) != name or "\\" in name or name in listed):
            raise ValueError("unsafe or duplicate manifest path")
        listed.add(name)
        path = root / name
        if (path.resolve() != path.absolute() or not path.is_file()
                or path.stat().st_size != entry["bytes"] or sha(path) != entry["sha256"]):
            raise ValueError(f"bundle integrity failure: {name}")
    actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
    if actual != listed | {"manifest.json", "SHA256SUMS"}:
        raise ValueError("bundle contains missing or unlisted files")
    sums = "".join(f"{entry['sha256']}  {entry['path']}\n" for entry in manifest["files"])
    sums += f"{sha(root / 'manifest.json')}  manifest.json\n"
    if (root / "SHA256SUMS").read_text() != sums:
        raise ValueError("SHA256SUMS does not match the manifest")
    relay_address(manifest["relay"])
    return manifest


def handler(root, manifest, relay, port):
    files = {"/" + f["path"].removeprefix("browser/"): root / f["path"]
             for f in manifest["files"] if f["path"].startswith("browser/")}
    config = json.dumps({"relays": [relay]}).encode()
    types = {".html": "text/html; charset=utf-8", ".js": "text/javascript",
             ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm",
             ".svg": "image/svg+xml", ".woff2": "font/woff2", ".txt": "text/plain"}

    class Handler(BaseHTTPRequestHandler):
        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            # Reject DNS rebinding and cross-origin reads; no operator API.
            if self.headers.get("Host") != f"127.0.0.1:{port}":
                self.send_error(403)
                return
            origin = self.headers.get("Origin")
            if origin and origin != f"http://127.0.0.1:{port}":
                self.send_error(403)
                return
            path = unquote(urlsplit(self.path).path)
            if path == "/local-relay.json":
                self.reply(config, "application/json")
                return
            if path == "/" or re.fullmatch(r"/(?:(?:blocks|accounts)(?:/[^/]+)?|pool|peers|start|key|send|snark-worker|block-producer)/?", path):
                path = "/index.html"
            file = files.get(path)
            if file is None:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", types.get(file.suffix, "application/octet-stream"))
            self.send_header("Content-Length", str(file.stat().st_size))
            self.end_headers()
            if self.command != "HEAD":
                with file.open("rb") as stream:
                    import shutil
                    shutil.copyfileobj(stream, self.wfile)

        def reply(self, body, content_type):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def end_headers(self):
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cross-Origin-Resource-Policy", "same-origin")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--relay", type=relay_address, help="replace the bundle's relay; no discovery")
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("port must be between 1 and 65535")
    root = Path(__file__).resolve().parent
    try:
        manifest = verify(root)
    except (OSError, ValueError, KeyError, argparse.ArgumentTypeError) as error:
        parser.exit(1, f"Verification failed: {error}\n")
    print("Bundle hashes verified. Authenticate the archive with the signed release checksums.", flush=True)
    if args.verify_only:
        return
    relay = args.relay or manifest["relay"]
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(root, manifest, relay, args.port))
    print(f"Open http://127.0.0.1:{args.port}/\nRelay: {relay}\nCtrl-C to stop.", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
