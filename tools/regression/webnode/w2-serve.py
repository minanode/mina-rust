"""Serve the W2 bundle with cross-origin isolation in a remote browser slot."""
import http.server
import os
import pathlib
import urllib.parse

root = pathlib.Path(os.environ["WEB_ROOT"]).resolve()
circuits = pathlib.Path(os.environ["CIRCUITS"]).resolve()


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(root), **kwargs)

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        super().end_headers()

    def do_GET(self):
        if self.path == "/w2-empty":
            body = b"<!doctype html><title>W2 baseline</title>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def translate_path(self, path):
        path = urllib.parse.urlparse(path).path
        prefix = "/assets/webnode/circuit-blobs/4.0.2mainnet/"
        if path.startswith(prefix):
            rel = path.removeprefix("/assets/webnode/circuit-blobs/")
            target = (circuits / rel).resolve()
            if circuits not in target.parents:
                raise ValueError("path traversal")
            return str(target)
        target = pathlib.Path(super().translate_path(path))
        if not target.exists() and "." not in pathlib.Path(path).name:
            return str(root / "index.html")
        return str(target)


http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ["STATIC_PORT"])), Handler).serve_forever()
