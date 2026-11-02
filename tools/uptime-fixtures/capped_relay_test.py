#!/usr/bin/env python3
"""Checks capped_relay.py against a loopback dummy upstream. Sends nothing out.

Usage: capped_relay_test.py SCRATCH_DIR (new or empty; ports 8861-8869).
"""

import concurrent.futures
import http.server
import json
from pathlib import Path
import sys
import threading
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from capped_relay import make_server  # noqa: E402

UA = "mina-rust/test"


def dummy_upstream(port, received, status=200):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            received.append({"path": self.path, "headers": dict(self.headers.items()), "body": body})
            reply = b'{"dummy":true}'
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("X-Dummy", "1")
            self.send_header("Content-Length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, *args):
            pass

    return http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)


def start(server):
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def post(port, body, method="POST"):
    request = urllib.request.Request(f"http://127.0.0.1:{port}/v1/submit", data=body,
                                     method=method, headers={"Content-Type": "application/json",
                                                             "User-Agent": UA})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=30) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def attempts(output):
    return [json.loads(line) for line in (output / "attempts.jsonl").read_text().splitlines()]


def main():
    scratch = Path(sys.argv[1])
    scratch.mkdir(parents=True, exist_ok=True)
    assert not any(scratch.iterdir()), "scratch directory must be empty"

    # Sequence: method, retry, gap and limit rules.
    received = []
    upstream = start(dummy_upstream(8861, received))
    output = scratch / "sequence"
    relay = start(make_server(8862, "http://127.0.0.1:8861/v1/submit", output, 2, 1.0))
    a, b, c, d = (b'{"report":"a"}', b'{"report":"b"}', b'{"report":"c"}',
                  b'{"report":"d"}')
    assert post(8862, None, "GET")[0] == 503
    assert post(8862, a) == (200, b'{"dummy":true}')
    assert post(8862, a) == (200, b'{"dummy":true}')  # a retry, answered locally
    assert post(8862, b)[0] == 503  # too soon: refused for good
    time.sleep(1.1)
    assert post(8862, b)[0] == 503  # its retry after the gap is still refused
    assert post(8862, c) == (200, b'{"dummy":true}')
    time.sleep(1.1)
    assert post(8862, d)[0] == 503  # limit
    assert post(8862, c) == (200, b'{"dummy":true}')  # retries still answered
    assert [r["body"] for r in received] == [a, c]
    for r in received:
        assert r["path"] == "/v1/submit"
        assert r["headers"]["Content-Type"] == "application/json"
        assert r["headers"]["User-Agent"] == UA
    log = attempts(output)
    assert [r["outcome"] for r in log] == [
        "refused", "forwarded", "replayed", "refused", "refused", "forwarded", "refused",
        "replayed"]
    assert [r.get("reason") for r in log] == [
        "only POST is forwarded", None, None, "too soon after the previous forward",
        "report refused earlier: too soon after the previous forward", None,
        "limit reached", None]
    assert [r["local_status"] for r in log] == [503, 200, 200, 503, 503, 200, 503, 200]
    for n, body in ((1, a), (2, c)):
        assert (output / f"forward-{n}.request.json").read_bytes() == body
        response = json.loads((output / f"forward-{n}.response.json").read_text())
        assert response["status"] == 200 and response["body_text"] == '{"dummy":true}'
        assert ["X-Dummy", "1"] in response["headers"]
    relay.shutdown()

    # A concurrent burst of distinct bodies: exactly two forwarded.
    received.clear()
    output = scratch / "burst"
    relay = start(make_server(8863, "http://127.0.0.1:8861/v1/submit", output, 2, 0.0))
    bodies = [json.dumps({"report": i}).encode() for i in range(40)]
    with concurrent.futures.ThreadPoolExecutor(20) as pool:
        statuses = list(pool.map(lambda body: post(8863, body)[0], bodies))
    assert sorted(statuses) == [200, 200] + [503] * 38, statuses
    assert len(received) == 2
    assert sum(r["forwarded"] for r in attempts(output)) == 2
    relay.shutdown()
    upstream.shutdown()

    # Upstream failures still count; their retries get the same local 504.
    output = scratch / "failing"
    relay = start(make_server(8864, "http://127.0.0.1:8865/v1/submit", output, 2, 0.0))
    assert post(8864, a)[0] == 504
    assert post(8864, a)[0] == 504
    assert post(8864, b)[0] == 504
    assert post(8864, c)[0] == 503
    log = attempts(output)
    assert [r["outcome"] for r in log] == ["forwarded", "replayed", "forwarded", "refused"]
    assert log[3]["reason"] == "limit reached"
    relay.shutdown()

    # A retriable and a terminal upstream status: each report reaches the
    # service once and its retries get the same status and body locally.
    for port, upstream_port, status in ((8867, 8866, 500), (8869, 8868, 401)):
        received.clear()
        upstream = start(dummy_upstream(upstream_port, received, status=status))
        output = scratch / f"status-{status}"
        relay = start(make_server(port, f"http://127.0.0.1:{upstream_port}/v1/submit",
                                  output, 2, 0.0))
        for _ in range(8):
            assert post(port, a) == (status, b'{"dummy":true}')
        assert len(received) == 1
        assert [r["outcome"] for r in attempts(output)] == ["forwarded"] + ["replayed"] * 7
        relay.shutdown()
        upstream.shutdown()
    print("capped_relay: all checks passed")


if __name__ == "__main__":
    main()
