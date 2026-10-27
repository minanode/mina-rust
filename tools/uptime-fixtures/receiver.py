#!/usr/bin/env python3
"""Loopback-only report recorder; never forwards requests or validates proofs."""

import argparse
import base64
import http.server
import json
from pathlib import Path
import time


def make_server(port, output, statuses=(200,)):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        attempts = {}

        def do_POST(self):
            if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                parts = []
                while True:
                    size = int(self.rfile.readline().split(b";", 1)[0], 16)
                    if not size:
                        while self.rfile.readline() != b"\r\n":
                            pass
                        break
                    parts.append(self.rfile.read(size))
                    if self.rfile.read(2) != b"\r\n":
                        raise ValueError("Invalid chunk terminator")
                body = b"".join(parts)
            else:
                body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            attempt = Handler.attempts.get(self.path, 0)
            status = statuses[min(attempt, len(statuses) - 1)]
            Handler.attempts[self.path] = attempt + 1
            record = {
                "received_at_unix": time.time(),
                "method": self.command,
                "path": self.path,
                "headers": list(self.headers.items()),
                "body_base64": base64.b64encode(body).decode(),
                "response_status": status,
            }
            with Path(output).open("a") as stream:
                stream.write(json.dumps(record) + "\n")
            reply = b'{"status":"local fixture receiver"}'
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, *args):
            pass

    return http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=3592)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--statuses", default="200", help="e.g. 503,503,200")
    args = parser.parse_args()
    server = make_server(args.port, args.output, tuple(map(int, args.statuses.split(","))))
    print(f"Recording on http://127.0.0.1:{server.server_port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
