#!/usr/bin/env python3
"""Loopback relay that forwards at most two distinct uptime reports.

Point a node's --uptime-url at this relay. A report is identified by the
SHA-256 of its request body. Only the first attempt of a report can be
forwarded, byte for byte, and only while both of these hold:

  1. fewer than --limit (at most 2) reports have been forwarded;
  2. at least --min-gap seconds have passed since the previous forward.

Otherwise the report is refused for good: this attempt and all its retries
get a local 503. A retry of a forwarded report is answered locally with the
status, content type and body the service gave its first attempt. The relay
never retries and never forwards anything else, so the service receives at
most --limit requests. The decision and the counter update happen together
under one lock, before the upstream request starts; a forward that fails
still counts (its retries get the local 504 it got).

Each forwarded body and the full upstream response (status, reason, headers,
body) are written to --output. Every attempt is logged to attempts.jsonl
with its time, body hash and outcome.
"""

import argparse
import base64
import hashlib
import http.client
import http.server
import json
from pathlib import Path
import threading
import time
import urllib.parse

MAX_LIMIT = 2
UPSTREAM_TIMEOUT = 50  # below the node's 60-second request timeout
MAX_BODY = 16 * 1024 * 1024


class Gate:
    """The forwarding decision. Nothing else may increment `forwarded`."""

    def __init__(self, limit, min_gap):
        assert 1 <= limit <= MAX_LIMIT
        self.limit = limit
        self.min_gap = min_gap
        self.lock = threading.Lock()
        self.forwarded = 0
        self.last_forward = None
        self.forwards = {}  # body hash -> forward number
        self.refused = {}  # body hash -> reason

    def admit(self, body_sha256, now):
        """Returns ("forward", n), ("replay", n) or ("refuse", reason)."""
        with self.lock:
            if body_sha256 in self.forwards:
                return "replay", self.forwards[body_sha256]
            if body_sha256 in self.refused:
                return "refuse", "report refused earlier: " + self.refused[body_sha256]
            if self.forwarded >= self.limit:
                reason = "limit reached"
            elif self.last_forward is not None and now - self.last_forward < self.min_gap:
                reason = "too soon after the previous forward"
            else:
                self.forwarded += 1
                self.last_forward = now
                self.forwards[body_sha256] = self.forwarded
                return "forward", self.forwarded
            self.refused[body_sha256] = reason
            return "refuse", reason


def make_server(port, upstream, output, limit, min_gap):
    url = urllib.parse.urlsplit(upstream)
    assert url.scheme in ("https", "http") and url.hostname
    target = url.path + ("?" + url.query if url.query else "")
    gate = Gate(limit, min_gap)
    log_lock = threading.Lock()
    output.mkdir(parents=True, exist_ok=True)
    attempt_counter = [0]
    # Forward number -> (status, content type, body) for its retries, and an
    # event set once that answer exists.
    answers = {n: [threading.Event(), None] for n in range(1, limit + 1)}

    def log(record):
        with log_lock:
            with (output / "attempts.jsonl").open("a") as stream:
                stream.write(json.dumps(record) + "\n")

    def forward(body, content_type, user_agent):
        connection_class = (http.client.HTTPSConnection if url.scheme == "https"
                            else http.client.HTTPConnection)
        connection = connection_class(url.hostname, url.port, timeout=UPSTREAM_TIMEOUT)
        try:
            headers = {"Content-Type": content_type, "Content-Length": str(len(body))}
            if user_agent is not None:
                headers["User-Agent"] = user_agent
            connection.request("POST", target, body=body, headers=headers)
            response = connection.getresponse()
            response_body = response.read(MAX_BODY + 1)
            return {"status": response.status, "reason": response.reason,
                    "headers": response.getheaders(),
                    "body_truncated": len(response_body) > MAX_BODY,
                    "body": response_body[:MAX_BODY]}
        finally:
            connection.close()

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def reply(self, status, body, content_type="application/json"):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True

        def refuse(self, record, reason):
            record.update(outcome="refused", forwarded=False, local_status=503, reason=reason)
            log(record)
            self.reply(503, json.dumps({"error": "relay: " + reason}).encode())

        def answer(self, record, number, status, content_type, body):
            event, _ = answers[number]
            answers[number][1] = (status, content_type, body)
            event.set()
            log(record)
            self.reply(status, body, content_type)

        def handle_any(self):
            with log_lock:
                attempt_counter[0] += 1
                attempt = attempt_counter[0]
            length = self.headers.get("Content-Length")
            body = b""
            if length is not None and 0 <= int(length) <= MAX_BODY:
                body = self.rfile.read(int(length))
            digest = hashlib.sha256(body).hexdigest()
            record = {"attempt": attempt, "received_at_unix": time.time(),
                      "received_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                      "method": self.command, "path": self.path,
                      "request_headers": list(self.headers.items()),
                      "body_bytes": len(body), "body_sha256": digest}
            if self.command != "POST":
                return self.refuse(record, "only POST is forwarded")
            if length is None or self.headers.get("Transfer-Encoding"):
                return self.refuse(record, "needs a Content-Length body")
            if len(body) != int(length):
                return self.refuse(record, "short body")
            action, value = gate.admit(digest, time.monotonic())
            if action == "refuse":
                return self.refuse(record, value)
            if action == "replay":
                event, _ = answers[value]
                event.wait()
                status, content_type, reply = answers[value][1]
                record.update(outcome="replayed", forwarded=False, forward=value,
                              local_status=status)
                log(record)
                return self.reply(status, reply, content_type)
            number = value
            prefix = output / f"forward-{number}"
            prefix.with_suffix(".request.json").write_bytes(body)
            started = time.time()
            record.update(outcome="forwarded", forwarded=True, forward=number,
                          upstream_started_unix=started)
            try:
                response = forward(body, self.headers.get("Content-Type", ""),
                                   self.headers.get("User-Agent"))
            except Exception as error:  # the forward still counts
                record.update(upstream_error=repr(error), local_status=504)
                return self.answer(record, number, 504, "application/json",
                                   json.dumps({"error": "relay: upstream failed"}).encode())
            finished = time.time()
            prefix.with_suffix(".response.json").write_text(json.dumps({
                "forward": number, "url": upstream,
                "started_unix": started, "finished_unix": finished,
                "status": response["status"], "reason": response["reason"],
                "headers": response["headers"], "body_truncated": response["body_truncated"],
                "body_base64": base64.b64encode(response["body"]).decode(),
                "body_text": response["body"].decode("utf-8", "replace")}, indent=2) + "\n")
            record.update(upstream_seconds=finished - started, upstream_status=response["status"],
                          upstream_body_sha256=hashlib.sha256(response["body"]).hexdigest(),
                          local_status=response["status"])
            content_type = dict((k.lower(), v) for k, v in response["headers"]).get(
                "content-type", "application/octet-stream")
            self.answer(record, number, response["status"], content_type, response["body"])

        do_POST = do_GET = do_PUT = do_HEAD = do_DELETE = handle_any

        def log_message(self, *args):
            pass

    return http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--upstream", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--limit", type=int, choices=range(1, MAX_LIMIT + 1), default=MAX_LIMIT)
    parser.add_argument("--min-gap", type=float, default=450.0)
    args = parser.parse_args()
    server = make_server(args.port, args.upstream, args.output, args.limit, args.min_gap)
    print(f"relay on 127.0.0.1:{args.port}, at most {args.limit} forwards to {args.upstream}",
          flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
