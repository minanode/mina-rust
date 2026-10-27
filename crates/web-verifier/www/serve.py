#!/usr/bin/env python3
"""Serve the demo with the cross-origin isolation headers threaded wasm needs.

    python3 crates/web-verifier/www/serve.py [port]
"""
import functools
import http.server
import pathlib
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".js": "text/javascript",
        ".wasm": "application/wasm",
        ".json": "application/json",
    }

    def end_headers(self):
        # SharedArrayBuffer (and so wasm threads) requires cross-origin isolation.
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8740
    root = pathlib.Path(__file__).resolve().parent
    handler = functools.partial(Handler, directory=str(root))
    with http.server.ThreadingHTTPServer(("127.0.0.1", port), handler) as server:
        print(f"serving {root} on http://127.0.0.1:{port}/", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
