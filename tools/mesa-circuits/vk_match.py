#!/usr/bin/env python3
"""Check whether every commitment coordinate of a live blockchain VK (GraphQL
JSON, data/vk/*.json) occurs, as 32 little-endian bytes, in an OCaml key-cache
verification key file. Prints matches per commitment family.

This is a presence test, not a positional one; the mesa_circuit_blobs example
of mina-tree compares by position. Exits 1 unless every coordinate is found, so
mismatched pairs (controls) exit 1 too.

Usage: vk_match.py <live-vk.json> <ocaml-vk-cache-file>
"""
import json
import sys

live = json.load(open(sys.argv[1]))
blob = open(sys.argv[2], "rb").read()


def coords(x):
    if isinstance(x, str):
        yield x
    elif isinstance(x, list):
        for y in x:
            yield from coords(y)


total = found = 0
for fam, val in live["commitments"].items():
    if val is None:
        continue
    cs = list(coords(val))
    hit = 0
    for c in cs:
        b = bytes.fromhex(c[2:].rjust(64, "0"))[::-1]
        if b in blob:
            hit += 1
    total += len(cs)
    found += hit
    print(f"{fam:28s} {hit}/{len(cs)}")
print("total", found, "/", total)
sys.exit(0 if total and found == total else 1)
