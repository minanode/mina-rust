#!/usr/bin/env python3
"""Check immutable OCaml bytes, encodings, schema examples, and local recorder."""

import base64
import hashlib
import http.client
import json
from pathlib import Path
import tempfile
import threading

from extract_capture import base58check
from receiver import make_server

ROOT = Path(__file__).resolve().parent


def main():
    count = 0
    for folder in sorted((ROOT / "fixtures").iterdir()):
        if not folder.is_dir():
            continue
        request_bytes = (folder / "uptime-1.json").read_bytes()
        request = json.loads(request_bytes)
        manifest = json.loads((folder / "manifest.json").read_text())
        key = json.loads((folder / "THROWAWAY-TEST-KEY.json").read_text())
        private = base58check(key["private_key_base58"])
        assert len(private) == 34 and private[:2] == bytes([90, 1])
        assert key["public_key"] == request["submitter"]
        data = (folder / "uptime-data.json").read_bytes()
        assert b'"data":' + data + b',"signature":' in request_bytes
        assert hashlib.blake2b(data, digest_size=32).hexdigest() == manifest["data_blake2b_256_hex"]
        assert len(request_bytes) == manifest["request_bytes"]
        block = base64.b64decode(request["data"]["block"], validate=True)
        assert len(block) == manifest["block_bytes"]
        assert hashlib.sha256(block).hexdigest() == manifest["block_sha256"]
        raw_signature = base58check(request["signature"])
        assert raw_signature[:2] == bytes([154, 1]) and len(raw_signature) == 66
        assert str(int.from_bytes(raw_signature[2:34], "little")) == manifest["signature_field_decimal"]
        assert str(int.from_bytes(raw_signature[34:], "little")) == manifest["signature_scalar_decimal"]
        public = base58check(request["submitter"])
        assert public[:3] == bytes([203, 1, 1]) and len(public) == 36
        status = json.loads((folder / "node-status-1.json").read_bytes())["data"]
        if manifest["simplified"]:
            assert "version" not in status and len(status) == 7
        else:
            assert status["version"] == 2 and len(status) == 23
        records = [json.loads(line) for line in (folder / "requests.jsonl").read_text().splitlines()]
        uptime = [r for r in records if r["path"] == "/uptime"]
        for record in records:
            body = base64.b64decode(record["body_base64"], validate=True)
            headers = {k.lower(): v for k, v in record["headers"]}
            assert record["method"] == "POST"
            assert headers["content-type"] == "application/json"
            assert int(headers["content-length"]) == len(body)
        if manifest.get("response_statuses") == "503,503,200":
            assert [r["response_status"] for r in uptime[:3]] == [503, 503, 200]
            assert all(r["response_status"] == 200 for r in uptime[3:])
            assert len({r["body_base64"] for r in uptime[:3]}) == 1
            intervals = [b["received_at_unix"] - a["received_at_unix"] for a, b in zip(uptime[:2], uptime[1:3])]
            assert all(3.5 < seconds < 6 for seconds in intervals)
            print(folder.name, "OCaml retry intervals:", intervals)
        count += 1
    example = json.loads((ROOT / "fixtures/node-error-source-derived.json").read_text())
    assert len(example["data"]) == 17 and example["data"]["version"] == 1
    assert len(json.dumps(example, separators=(",", ":")).encode()) < 10000
    # Verify status injection, exact-byte capture and resource cleanup locally.
    with tempfile.TemporaryDirectory(prefix="r43e1-receiver-check-") as temp:
        output = Path(temp) / "requests.jsonl"
        server = make_server(0, output, (503, 200))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for expected in (503, 200):
                connection = http.client.HTTPConnection("127.0.0.1", server.server_port)
                connection.request("POST", "/uptime", b'{"offline":true}',
                                   {"Content-Type": "application/json"})
                response = connection.getresponse()
                assert response.status == expected
                response.read()
                connection.close()
            records = [json.loads(line) for line in output.read_text().splitlines()]
            assert len(records) == 2
            assert all(base64.b64decode(r["body_base64"]) == b'{"offline":true}' for r in records)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
    print(f"{count} OCaml fixture sets and loopback receiver checks passed")


if __name__ == "__main__":
    main()
