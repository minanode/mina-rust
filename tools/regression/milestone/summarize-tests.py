#!/usr/bin/env python3
"""Summarise the wave 4b test runs: per mode and suite, then per package for
the ordinary set plus I18's scenario suites (the m8 total), and the rest
(the m8 gates' summarize-tests.py).

Usage: python3 -I summarize-tests.py [--json OUT]
"""
import collections
import json
import os
import pathlib
import sys

g = pathlib.Path(os.environ['GATE_DIR']).resolve() / 'tests'
modes = sorted(p.parent.name for p in g.glob('*/results.jsonl'))
all_rows = {}
for mode in modes:
    rows = [json.loads(l) for l in open(g / mode / 'results.jsonl')]
    all_rows[mode] = rows
    p = f = i = 0
    for r in rows:
        if r['summary']:
            _, a, b, c = r['summary'][0]
            p += int(a); f += int(b); i += int(c)
    bad = [r['suite'] for r in rows if r['exit'] != 0 or not r['summary']]
    secs = sum(r['seconds'] for r in rows)
    print(f'{mode:32} suites={len(rows):3} passed={p:4} failed={f:3} ignored={i:3} '
          f'seconds={secs:8.1f} bad={bad}')
    for r in rows:
        if r['failed']:
            print('   FAILED', r['suite'], r['failed'])
per_pkg = collections.defaultdict(lambda: [0, 0, 0, 0])
for mode, rows in all_rows.items():
    if mode != 'ordinary' and not mode.startswith('scenario-'):
        continue
    for r in rows:
        key = 'mina-node-testing (scenarios)' if mode.startswith('scenario-') else r['package']
        per_pkg[key][3] += 1
        if r['summary']:
            _, a, b, c = r['summary'][0]
            per_pkg[key][0] += int(a); per_pkg[key][1] += int(b); per_pkg[key][2] += int(c)
print()
print('| Package | Suites | Passed | Failed | Ignored |')
print('| --- | ---: | ---: | ---: | ---: |')
tot = [0, 0, 0, 0]
for k, (a, b, c, n) in per_pkg.items():
    print(f'| `{k}` | {n} | {a} | {b} | {c} |')
    tot = [tot[0] + a, tot[1] + b, tot[2] + c, tot[3] + n]
print(f'| **All {tot[3]} suites** | {tot[3]} | **{tot[0]}** | **{tot[1]}** | **{tot[2]}** |')
if len(sys.argv) > 2 and sys.argv[1] == '--json':
    json.dump(all_rows, open(sys.argv[2], 'w'), indent=1)

if not all_rows or any(r['exit'] != 0 or not r['summary']
                       for rows in all_rows.values() for r in rows):
    raise SystemExit('missing or failed gate results')
