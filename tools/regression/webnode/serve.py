#!/usr/bin/env python3
"""Serve a web node build (build.sh's OUT/browser) with the headers its WASM
threads need, where `npx serve@14 -c serve.json` isn't available, as on the
run machine (tools/remote). The headers are serve.json's for every path
("**"), so the page runs under the build's own Content-Security-Policy. Paths
without a file extension that aren't files get index.html, as `serve -s`
does.

    python3 serve.py ROOT PORT [HOST]

HOST defaults to 127.0.0.1. With CSP_REPORTS=FILE the policy also names a
report endpoint on this server, which appends every violation report (one
JSON object per line) to FILE, so that a check can fail on any.
"""

import http.server
import json
import os
import sys
import threading
import urllib.parse
from pathlib import Path

REPORT_PATH = "/__csp-report"


def page_headers(root):
    """serve.json's headers for every path, in order."""
    config = json.loads((root / "serve.json").read_text())
    headers = []
    for rule in config.get("headers", []):
        if rule.get("source") == "**":
            headers += [(h["key"], h["value"]) for h in rule["headers"]]
    return headers


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    root = Path(sys.argv[1]).resolve()
    port = int(sys.argv[2])
    host = sys.argv[3] if len(sys.argv) == 4 else "127.0.0.1"
    reports = os.environ.get("CSP_REPORTS")
    headers = page_headers(root)
    if reports:
        headers = [
            (k, f"{v}; report-uri {REPORT_PATH}")
            if k.lower() == "content-security-policy"
            else (k, v)
            for k, v in headers
        ]
    lock = threading.Lock()

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(root), **kwargs)

        def end_headers(self):
            for key, value in headers:
                self.send_header(key, value)
            super().end_headers()

        def do_POST(self):
            if not reports or self.path != REPORT_PATH:
                self.send_error(405)
                return
            length = min(int(self.headers.get("Content-Length") or 0), 65536)
            body = self.rfile.read(length)
            try:
                report = json.loads(body)
            except ValueError:
                report = {"unparsed": body.decode("utf-8", "replace")}
            with lock, open(reports, "a") as f:
                f.write(json.dumps(report) + "\n")
            self.send_response(204)
            self.end_headers()

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
