#!/usr/bin/env python3
"""Check whether the commitment x-coordinates of a mina-rust embedded verifier
index (crates/ledger/src/proofs/data/*_verifier_index.json, ark compressed
points: 32-byte little-endian x plus a flags byte) occur in an OCaml key-cache
verification key file.

Usage: vk_match_rust.py <rust-verifier-index.json> <ocaml-vk-cache-file>
"""
import json
import sys

idx = json.load(open(sys.argv[1]))
blob = open(sys.argv[2], "rb").read()
total = found = 0
for fam, val in idx.items():
    if not fam.endswith("_comm") or val is None:
        continue
    comms = val if isinstance(val, list) else [val]
    hit = n = 0
    for c in comms:
        for chunk in c["chunks"]:
            x = bytes.fromhex(chunk)[:32]
            n += 1
            if x in blob:
                hit += 1
    total += n
    found += hit
    print(f"{fam:28s} {hit}/{n}")
print("total", found, "/", total)
