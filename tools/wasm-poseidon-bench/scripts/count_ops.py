#!/usr/bin/env python3
"""Count WASM instructions in the functions of a WAT listing whose name
contains every given fragment.

Usage: wasm-tools print module.wasm | count_ops.py FRAGMENT [FRAGMENT...]

Prints, for each matching function, its name (shortened) and the most common
instructions. Used to compare arkworks' Fp multiplication in the web node build
with the standalone benchmark build.
"""

import collections
import re
import sys

OP = re.compile(r"^\s*([a-z0-9_]+\.[a-z0-9_]+|call|br_if|loop|local\.[a-z]+)")


def main(fragments):
    current = None
    counts = None
    results = []
    for line in sys.stdin:
        if line.startswith("  (func "):
            if current is not None:
                results.append((current, counts))
            name = line.strip()[len("(func "):]
            current = name if all(f in name for f in fragments) else None
            counts = collections.Counter()
            continue
        if current is None:
            continue
        m = OP.match(line)
        if m:
            counts[m.group(1)] += 1
    if current is not None:
        results.append((current, counts))
    for name, c in results:
        total = sum(v for k, v in c.items() if not k.startswith("local."))
        print(f"{name[:160]}\n  total (without local.*) {total}")
        for k in ("i64.mul", "i64.add", "i64.lt_u", "i64.shr_u", "i64.shl", "call"):
            print(f"  {k:12} {c.get(k, 0)}")


if __name__ == "__main__":
    main(sys.argv[1:])
