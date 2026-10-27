#!/usr/bin/env python3
"""Sizes, gzip sizes and sha256 of the dumped circuit blobs in a directory."""
import glob
import gzip
import hashlib
import os
import sys

d = sys.argv[1]
files = sorted(
    f
    for pat in ("*_gates.json", "*_internal_vars.bin", "*_rows_rev.bin")
    for f in glob.glob(os.path.join(d, pat))
)
raw = gz = 0
for f in files:
    data = open(f, "rb").read()
    z = len(gzip.compress(data, 9))
    raw += len(data)
    gz += z
    print(f"{hashlib.sha256(data).hexdigest()[:16]} {len(data):>10} {z:>9} {os.path.basename(f)}")
print(f"files {len(files)} raw {raw} gzip {gz}")
