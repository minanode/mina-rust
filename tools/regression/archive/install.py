#!/usr/bin/env python3
"""Extract the exact noble amd64 archive package recorded in pin.json."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--scratch', type=Path, required=True)
    args = parser.parse_args()
    pin = json.loads(Path(__file__).with_name('pin.json').read_text())
    if args.prefix.exists():
        parser.error('prefix must not exist; existing installs are never overwritten')
    args.scratch.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(args.scratch).free < 57 * 1024**3:
        parser.error('requires 57 GiB free, including a 55 GiB reserve')
    with tempfile.TemporaryDirectory(dir=args.scratch) as tmp:
        package = Path(tmp) / 'archive.deb'
        with urllib.request.urlopen(pin['package_url'], timeout=60) as source, package.open('wb') as dest:
            shutil.copyfileobj(source, dest)
        with package.open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != pin['package_sha256']:
                raise RuntimeError('archive package SHA-256 mismatch')
        subprocess.run(['dpkg-deb', '-x', package, args.prefix], check=True)
    for relative, expected in pin['files'].items():
        with (args.prefix / relative).open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != expected:
                raise RuntimeError(f'installed file SHA-256 mismatch: {relative}')
    (args.prefix / 'README.md').write_text(
        '# Mina archive ' + pin['version'] + '\n\n'
        'Official noble amd64 package, extracted without a system install.\n'
        'Source commit: ' + pin['commit'] + '\n'
        'Package: ' + pin['package_url'] + '\n'
        'SHA-256: ' + pin['package_sha256'] + '\n\n'
        'SQL is in etc/mina/archive. Requires libpq, libjemalloc and the\n'
        'package runtime dependencies. Set MINA_PROFILE for the network and\n'
        'TMPDIR to a private directory. See tools/regression/archive/README.md.\n')


if __name__ == '__main__':
    main()
