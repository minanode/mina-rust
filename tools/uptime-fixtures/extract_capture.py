#!/usr/bin/env python3
"""Extract OCaml captures and their GENERATED TEST key into reviewable fixtures."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import re

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def base58check(text):
    number = 0
    for char in text:
        number = number * 58 + ALPHABET.index(char)
    raw = number.to_bytes((number.bit_length() + 7) // 8, "big")
    raw = b"\0" * (len(text) - len(text.lstrip("1"))) + raw
    assert hashlib.sha256(hashlib.sha256(raw[:-4]).digest()).digest()[:4] == raw[-4:]
    return raw[:-4]


def extract(work, output):
    output.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((work / "manifest.json").read_text())
    # ONLY this capture script's newly generated key is accepted. Do not use
    # extract() with an operator directory or add an arbitrary key-path argument.
    assert manifest["private_chain"] and manifest["source_commit"] == "84b8d96954b218f372b93f65a071abf528e6bb61"
    key = re.search(r"^Private key: (EK[1-9A-HJ-NP-Za-km-z]+)$",
                    (work / "THROWAWAY-keypair.txt").read_text(), re.MULTILINE)
    assert key
    private = base58check(key[1])
    assert len(private) == 34 and private[:2] == bytes([90, 1])
    keypair = {"warning": "GENERATED FOR R43e1 OFFLINE TESTS ONLY. NEVER FUND OR USE ON A NETWORK.",
               "private_key_base58": key[1],
               "public_key": (work / "keys/submitter.pub").read_text().strip()}
    (output / "THROWAWAY-TEST-KEY.json").write_text(json.dumps(keypair, indent=2) + "\n")
    for name in ("requests.jsonl", "daemon.json"):
        (output / name).write_bytes((work / name).read_bytes())
    first = {}
    for line in (work / "requests.jsonl").read_text().splitlines():
        record = json.loads(line)
        name = record["path"].strip("/")
        if name not in first:
            first[name] = base64.b64decode(record["body_base64"])
            (output / f"{name}-1.json").write_bytes(first[name])
    request = json.loads(first["uptime"])
    data = json.dumps(request["data"], separators=(",", ":"), ensure_ascii=False).encode()
    # Reconstruct precisely the captured substring, rather than assuming a
    # serializer's field ordering or whitespace is equivalent.
    assert b'"data":' + data + b',"signature":' in first["uptime"]
    (output / "uptime-data.json").write_bytes(data)
    raw = base58check(request["signature"])
    assert raw[:2] == bytes([154, 1]) and len(raw) == 66
    manifest.update({
        "data_blake2b_256_hex": hashlib.blake2b(data, digest_size=32).hexdigest(),
        "signature_field_decimal": str(int.from_bytes(raw[2:34], "little")),
        "signature_scalar_decimal": str(int.from_bytes(raw[34:], "little")),
        "block_sha256": hashlib.sha256(base64.b64decode(request["data"]["block"])).hexdigest(),
        "block_bytes": len(base64.b64decode(request["data"]["block"])),
        "request_bytes": len(first["uptime"]),
        "snark_work_present": "snark_work" in request["data"],
        "origin": "Unmodified official OCaml daemon HTTP output; private proof-none chain, no peers or producer",
    })
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Extracted {len(first)} report kinds into {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    extract(args.capture, args.output)
