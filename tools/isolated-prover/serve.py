#!/usr/bin/env python3
"""Serve only this prototype, its fixture and the explicitly supplied blobs."""
import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


class Handler(SimpleHTTPRequestHandler):
    def translate_path(self, path):
        path = unquote(urlsplit(path).path)
        if path == "/fixture.bin":
            return str(REPO / "crates/ledger/src/proofs/data/mesa_mainnet_payment_base_job.bin")
        if path == "/block-fixture.bin":
            return str(REPO / "crates/ledger/src/proofs/data/mesa_mainnet_genesis_block_input.bin")
        prefix = "/assets/webnode/circuit-blobs/"
        if path.startswith(prefix):
            relative = Path(path[len(prefix):])
            if ".." not in relative.parts and not relative.is_absolute():
                # The generated circuit directory deliberately contains symlinks.
                return str(self.server.blobs / relative)
            return str(HERE / "missing")
        return super().translate_path(path)

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--blobs", type=Path, required=True)
    args = parser.parse_args()
    if not (args.blobs / "4.0.2mainnet").is_dir():
        parser.error("--blobs must contain 4.0.2mainnet")
    server = ThreadingHTTPServer(("127.0.0.1", 4228), partial(Handler, directory=str(HERE / "www")))
    server.blobs = args.blobs.resolve()
    print("I34 server ready on http://127.0.0.1:4228", flush=True)
    server.serve_forever()
