#!/usr/bin/env python3
"""Check capture integrity and recorded OCaml results; not a proof verifier."""

import base64
import hashlib
import json
from pathlib import Path
import struct

from extract_capture import base58check

ROOT = Path(__file__).parent / "full-proof"


def read(path):
    return json.loads(path.read_bytes())


def main():
    evidence = read(ROOT / "verification-results.json")
    assert evidence["validation_enabled"] and evidence["exit_code"] == 0
    results = {r["case"]: r for r in evidence["results"]}
    count = 0
    for path in sorted(ROOT.iterdir()):
        if not path.is_dir():
            continue
        raw = (path / "uptime-1.json").read_bytes()
        request, manifest = json.loads(raw), read(path / "manifest.json")
        data = (path / "uptime-data.json").read_bytes()
        assert b'"data":' + data + b',"signature":' in raw
        assert json.loads(data) == request["data"]
        assert hashlib.sha256(raw).hexdigest() == manifest["request_sha256"]
        assert hashlib.blake2b(data, digest_size=32).hexdigest() == manifest["data_blake2b_256_hex"]
        assert len(raw) == manifest["request_bytes"]
        http, decoded = read(path / "http.json"), read(path / "decoded.json")
        assert base64.b64decode(http["body_base64"], validate=True) == raw
        assert http["path"] == "/uptime" and http["response_status"] == 200
        headers = {k.lower(): v for k, v in http["headers"]}
        assert headers["host"] == "127.0.0.1:8803"
        assert int(headers["content-length"]) == len(raw)
        assert request["submitter"] == decoded["submitter"] == manifest["submitter"]
        signature = base58check(request["signature"])
        assert signature[:2] == bytes([154, 1]) and len(signature) == 66
        assert str(int.from_bytes(signature[2:34], "little")) == manifest["signature_field_decimal"]
        assert str(int.from_bytes(signature[34:], "little")) == manifest["signature_scalar_decimal"]
        for field in ("block", "snark_work"):
            blob = base64.b64decode(request["data"][field], validate=True)
            assert len(blob) == manifest[field + "_bytes"]
            assert hashlib.sha256(blob).hexdigest() == manifest[field + "_sha256"]
        layout = read(path / "wire-layout.json")
        assert layout["outer_field_order"] == list(request)
        assert layout["data_field_order"] == list(request["data"])
        assert layout["request_bytes"] == len(raw) and not layout["trailing_newline"]
        for field in layout["fields"]:
            value = request
            for key in field["path"].split("."):
                value = value[key]
            span = raw[field["value_start"]:field["value_end_exclusive"]]
            assert len(span) == field["value_bytes"] and json.loads(span) == value
        work = decoded["snark_work"]
        assert decoded["block_roundtrip_exact"] and work["roundtrip_exact"]
        assert work["sok_matches"] and not work["sok_is_default"]
        assert work["target_matches_block"]
        assert work["fee"] == "0.001"
        # These byte offsets and expected values were measured by OCaml.
        raw_work = base64.b64decode(request["data"]["snark_work"])
        tail = raw_work[work["ledger_proof_bytes"]:]
        assert len(tail) == 13 and tail[8:] == bytes.fromhex("fd40420f00")
        assert struct.unpack("<d", tail[:8])[0] == work["proof_time_seconds"]
        sok = bytes.fromhex(work["sok_message_bin_prot_hex"])
        assert hashlib.blake2b(sok, digest_size=32).hexdigest() == work["proof_sok_digest_hex"]
        assert work["proof_sok_digest_hex"] == work["expected_sok_digest_hex"]
        selection, job = read(path / "selection.json"), read(path / "selected-job.json")
        assert hashlib.sha256((path / "selected-job.bin").read_bytes()).hexdigest() == manifest["selected_job_bin_sha256"]
        first = next(c for c in selection["candidates"] if c["matches"])
        assert first == selection["selected"] and first["kind"] == "Transition"
        for candidate in selection["candidates"]:
            assert candidate["matches"] == (candidate["kind"] == "Transition" and
                candidate["target_second_pass_ledger"] == decoded["staged_ledger_ledger_hash"])
        assert job[0] == "Transition" and job[1] == work["statement"]
        assert job[1]["target"]["second_pass_ledger"] == decoded["staged_ledger_ledger_hash"]
        result = results[path.name]
        assert result["validation_error"] is None
        assert result["height"] == decoded["height"] == manifest["height"]
        assert result["state_hash"] == decoded["state_hash"] == manifest["state_hash"]
        assert not (path / "THROWAWAY-TEST-KEY.json").exists()
        print(path.name, "capture bytes, SOK, selection and recorded verification passed")
        count += 1
    assert count > 0
    for name in ("wrong-submitter", "truncated-work", "corrupt-proof"):
        assert results["control-" + name]["validation_error"]
    for name in ("absent-work", "unrelated-work"):
        assert results["control-" + name]["validation_error"] is None
    print(f"{count} captures checked; controls are derived, not captured daemon cases")


if __name__ == "__main__":
    main()
