#!/usr/bin/env python3
"""Serve the I34c harness, the web node build, the circuit blobs and one
private run, on 127.0.0.1 only, cross-origin isolated.

/                               the harness page (www/)
/pkg/...                        the web node's wasm-bindgen output (--pkg)
/assets/webnode/circuit-blobs/  the circuit blobs (--blobs)
/run/genesis.bin                the run's prebuilt genesis config
/run/producer.json              the run's throwaway producer key and password
"""
import argparse
import json
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

HERE = Path(__file__).resolve().parent


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
    }

    def translate_path(self, path):
        path = unquote(urlsplit(path).path)
        for prefix, base in (("/pkg/", self.server.pkg),
                             ("/assets/webnode/circuit-blobs/", self.server.blobs)):
            if path.startswith(prefix):
                relative = Path(path[len(prefix):])
                if ".." in relative.parts or relative.is_absolute():
                    return str(HERE / "missing")
                return str(base / relative)
        if path == "/run/genesis.bin":
            return str(self.server.run / "genesis.bin")
        return super().translate_path(path)

    def do_GET(self):
        if urlsplit(self.path).path == "/run/producer.json":
            keys = self.server.run / "keys"
            body = json.dumps({
                "encrypted": json.loads((keys / "producer").read_text()),
                "password": (keys / "password").read_text(),
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--pkg", type=Path, required=True)
    parser.add_argument("--blobs", type=Path, required=True)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--port", type=int, default=4229)
    args = parser.parse_args()
    if not (args.blobs / "4.0.2mainnet").is_dir():
        parser.error("--blobs must contain 4.0.2mainnet")
    if not (args.pkg / "mina_node_web.js").is_file():
        parser.error("--pkg must hold mina_node_web.js")
    server = ThreadingHTTPServer(("127.0.0.1", args.port),
                                 partial(Handler, directory=str(HERE / "www")))
    server.pkg = args.pkg.resolve()
    server.blobs = args.blobs.resolve()
    server.run = args.run.resolve()
    print(f"I34c server ready on http://127.0.0.1:{args.port}", flush=True)
    server.serve_forever()
