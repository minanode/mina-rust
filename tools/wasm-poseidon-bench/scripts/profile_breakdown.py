#!/usr/bin/env python3
"""Aggregate self and inclusive time of Chrome .cpuprofile files.

Usage: profile_breakdown.py profile-*.cpuprofile

Prints the functions with the most self time over all profiles (idle and
program samples excluded), and for a few name fragments, the share of samples
whose stack contains a frame matching it.
"""

import collections
import json
import re
import sys

# Frames that are a thread waiting for work, not doing it.
WAITING = [
    "Sleep>::sleep",
    "thread::functions::park",
    "Condvar>::wait",
    "Stealer<",
    "Injector<",
    "wake_any_threads",
    "wait_until_cold",
    "waitAsync",
]

KEYS = [
    "poseidon",
    "hash_with_kimchi",
    "poseidon_block_cipher",
    "full_round",
    "sbox",
    "apply_mds",
    "MontBackend",
    "mul_assign",
    "square_in_place",
    "__multi3",
    "hash_fields",
    "to_fields",
]


def main(files):
    self_time = collections.Counter()
    inclusive = collections.Counter()
    total = 0
    for f in files:
        with open(f) as fh:
            p = json.load(fh)
        nodes = {n["id"]: n for n in p["nodes"]}
        parent = {}
        for n in p["nodes"]:
            for c in n.get("children", []):
                parent[c] = n["id"]
        for nid, c in collections.Counter(p["samples"]).items():
            name = nodes[nid]["callFrame"]["functionName"] or "(anonymous)"
            if name in ("(idle)", "(program)", "(garbage collector)"):
                continue
            if any(w in name for w in WAITING):
                continue
            name = re.sub(r"::h[0-9a-f]{16}", "", name)
            self_time[name] += c
            total += c
            seen = set()
            x = nid
            while x is not None:
                nm = nodes[x]["callFrame"]["functionName"]
                for key in KEYS:
                    if key in nm and key not in seen:
                        seen.add(key)
                        inclusive[key] += c
                x = parent.get(x)
    print("total non-idle samples", total)
    for n, c in self_time.most_common(30):
        print(f"{100 * c / total:5.1f}% {c:7d} {n[:400]}")
    print("--- stack contains ---")
    for k, c in inclusive.most_common():
        print(f"{100 * c / total:5.1f}% {k}")


if __name__ == "__main__":
    main(sys.argv[1:])
