#!/usr/bin/env python3
"""Summarise run-all.sh output: per platform and kernel, the median of the
per-run medians and the lowest single sample, in ns per operation.

Usage: summarize.py /tmp/r8/runs
"""

import collections
import re
import statistics
import sys
from pathlib import Path

LINE = re.compile(r"^(?P<name>.+?)\s{2,}(?P<ns>[0-9.]+)(?: ns/op\s+\(min (?P<min>[0-9.]+))?")


def main(directory):
    medians = collections.defaultdict(list)
    mins = collections.defaultdict(list)
    order = []
    for f in sorted(Path(directory).glob("*.txt")):
        platform = f.stem.rsplit("-", 1)[0] if f.stem[-1].isdigit() else f.stem
        module = "native" if platform.startswith("native") else None
        for line in f.read_text().splitlines():
            if line.startswith("== "):
                module = line.split()[1]
                continue
            if module is None or line.startswith(("check", "kernel", " ")):
                continue
            m = LINE.match(line)
            if not m:
                continue
            key = (platform, module, m.group("name").strip())
            if key not in medians:
                order.append(key)
            medians[key].append(float(m.group("ns")))
            mins[key].append(float(m.group("min") or m.group("ns")))
    current = None
    for key in order:
        if key[:2] != current:
            current = key[:2]
            print(f"\n{current[0]} / {current[1]}")
            print(f"  {'kernel':52} {'median':>10} {'min':>10} runs")
        med = statistics.median(medians[key])
        print(f"  {key[2]:52} {med:10.1f} {min(mins[key]):10.1f} {len(medians[key])}")


if __name__ == "__main__":
    main(sys.argv[1])
