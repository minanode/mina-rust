#!/usr/bin/env python3
"""Serve a web node build (build.sh's OUT/browser) with the headers its WASM
threads need, where `npx serve@14 -c serve.json` isn't available, as on the
run machine (tools/remote). Paths without a file extension that aren't files
get index.html, as `serve -s` does.

    python3 serve.py ROOT PORT [HOST]

HOST defaults to 127.0.0.1.
"""

import http.server
import sys
import urllib.parse
from pathlib import Path


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    root = Path(sys.argv[1]).resolve()
    port = int(sys.argv[2])
    host = sys.argv[3] if len(sys.argv) == 4 else "127.0.0.1"

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(root), **kwargs)

        def end_headers(self):
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

        def translate_path(self, path):
            translated = super().translate_path(path)
            name = Path(urllib.parse.urlparse(path).path).name
            if not Path(translated).exists() and "." not in name:
                return str(root / "index.html")
            return translated

        def log_message(self, fmt, *args):
            print(fmt % args, flush=True)

    http.server.ThreadingHTTPServer((host, port), Handler).serve_forever()


if __name__ == "__main__":
    main()
