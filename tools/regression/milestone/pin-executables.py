#!/usr/bin/env python3
"""Pin the m9 test executables (and the mina binary the status test runs) into
the gate's tests/bin/, with sha256 and size, so that later builds cannot
replace them while they run (wave 4a's pin-executables.py). Copies work across filesystems and survive later builds.

Usage: python3 -I pin-executables.py BUILD.jsonl...
"""
import hashlib
import json
import os
import shutil
import pathlib
import sys

g = pathlib.Path(os.environ['GATE_DIR']).resolve()
out = g / 'tests/bin'
out.mkdir(parents=True, exist_ok=True)
files = set()
for name in sys.argv[1:]:
    finished = False
    for s in (g / name).read_text().splitlines():
        try:
            a = json.loads(s)
        except ValueError:
            continue
        if a.get('reason') == 'build-finished':
            finished = a['success']
        if a.get('reason') == 'compiler-artifact' and a.get('executable') and a.get('profile', {}).get('test'):
            files.add(pathlib.Path(a['executable']))
    assert finished, name
files.add(pathlib.Path(os.environ['MINA_BIN']).resolve())
manifest = g / 'tests/pinned-executables.json'
records = json.loads(manifest.read_text()) if manifest.exists() else []
for src in sorted(files):
    dst = out / src.name
    assert not dst.exists(), dst
    shutil.copy2(src, dst)
    h = hashlib.sha256(dst.read_bytes()).hexdigest()
    records.append(dict(source=str(src), copy=str(dst), sha256=h, size=dst.stat().st_size))
manifest.write_text(json.dumps(records, indent=2) + '\n')
print('Pinned total', len(records), 'executables;', sum(r['size'] for r in records), 'bytes')
