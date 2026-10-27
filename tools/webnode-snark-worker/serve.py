#!/usr/bin/env python3
"""Serve the P5.3 harness pages, the web node build, the circuit blobs,
OCaml's proof fixtures and one private run, cross-origin isolated, on one
address only (127.0.0.1 by default).

/                               the harness pages (www/)
/pkg/...                        the web node's wasm-bindgen output (--pkg)
/assets/webnode/circuit-blobs/  the circuit blobs (--blobs)
/fixtures/...                   crates/ledger/src/proofs/data (public fixtures)
/run/genesis.bin                the run's prebuilt genesis config (--run)
/run/worker.json                the run's public worker settings (--run)

No key is served: a SNARK worker takes only a public key.
"""
import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parents[1] / "crates/ledger/src/proofs/data"


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
    }

    def translate_path(self, path):
        path = unquote(urlsplit(path).path)
        bases = [("/pkg/", self.server.pkg),
                 ("/assets/webnode/circuit-blobs/", self.server.blobs),
                 ("/fixtures/", FIXTURES)]
        if self.server.run is not None:
            bases.append(("/run/", self.server.run / "public"))
        for prefix, base in bases:
            if path.startswith(prefix):
                relative = Path(path[len(prefix):])
                if ".." in relative.parts or relative.is_absolute():
                    return str(HERE / "missing")
                return str(base / relative)
        return super().translate_path(path)

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, format, *args):
        # Requests only: the console of the run is the page's.
        if self.server.verbose:
            super().log_message(format, *args)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--pkg", type=Path, required=True)
    parser.add_argument("--blobs", type=Path, required=True)
    parser.add_argument("--run", type=Path,
                        help="a run directory whose public/ holds genesis.bin and worker.json")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()
    if not any((args.blobs / d).is_dir() for d in ("4.0.2devnet", "4.0.2mainnet")):
        parser.error("--blobs must contain 4.0.2devnet or 4.0.2mainnet")
    if not (args.pkg / "mina_node_web.js").is_file():
        parser.error("--pkg must hold mina_node_web.js")
    server = ThreadingHTTPServer((args.host, args.port),
                                 partial(Handler, directory=str(HERE / "www")))
    server.pkg = args.pkg.resolve()
    server.blobs = args.blobs.resolve()
    server.run = args.run.resolve() if args.run else None
    server.verbose = args.verbose
    print(f"P5.3 server ready on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()
