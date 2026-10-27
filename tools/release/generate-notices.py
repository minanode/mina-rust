#!/usr/bin/env python3
"""Regenerate release notices, exact upstream texts, and MPL source."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tomllib
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOCK = ROOT / 'tools/regression/lock.sh'
BUNDLE = ROOT / 'THIRD_PARTY_LICENSES'
SOURCE = ROOT / 'SOURCES' / 'option-ext-0.2.0.crate'


def cargo(*args):
    return subprocess.check_output(
        [str(LOCK), 'build', '--', 'nice', '-n', '10', 'cargo', *args],
        cwd=ROOT, text=True, env={**os.environ, 'CARGO_BUILD_JOBS': '8'},
    )


def files_at(path):
    return sorted(p for p in path.iterdir() if p.is_file() and
                  re.match(r'^(LICEN[CS]E|COPYING|NOTICE)([._-]|$)', p.name, re.I))


def notices_below(path):
    return sorted(p for p in path.rglob('*') if p.is_file() and p.parent != path
                  and re.match(r'^NOTICE([._-]|$)', p.name, re.I))


def add_text(texts, owners, source, label):
    data = source.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    texts.setdefault(digest, data)
    owners[digest].append(f'{label}: {source.name}')


def npm_package_dir(name):
    path = ROOT / 'ui' / 'node_modules' / name
    if not path.is_dir():
        raise SystemExit(f'installed dashboard package missing: {name}')
    return path


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dashboard-built', action='store_true',
                    help='reuse ui/dashboard/dist from an immediately preceding npm build')
args = parser.parse_args()
if not args.dashboard_built:
    subprocess.run(['npm', 'ci', '--no-audit', '--no-fund'], cwd=ROOT / 'ui', check=True)
    subprocess.run(['npm', 'run', 'build:dashboard'], cwd=ROOT / 'ui', check=True)

with (ROOT / 'Cargo.toml').open('rb') as source:
    version = tomllib.load(source)['workspace']['package']['version']
metadata = json.loads(cargo('metadata', '--locked', '--format-version', '1'))
packages = {(p['name'], p['version']): p for p in metadata['packages']}
unlicensed_local = [p['name'] for p in metadata['packages']
                    if p['source'] is None and not p['license']]
if unlicensed_local:
    raise SystemExit(f'local crates without a licence: {", ".join(unlicensed_local)}')
names = set()
for target in ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'):
    tree = cargo('tree', '--locked', '-p', 'mina-cli', '-e', 'normal',
                 '--features', 'vendored-openssl', '--target', target,
                 '--prefix', 'none', '--format', '{p}')
    for line in tree.splitlines():
        match = re.match(r'^([^ ]+) v([^ ]+)', line)
        if match:
            names.add(match.groups())

# openssl-src is a build dependency, but its OpenSSL source is linked in.
for extra in (('openssl-src', '300.4.1+3.4.0'),):
    if extra not in packages:
        raise SystemExit(f'missing linked native dependency: {extra}')
    names.add(extra)

texts = {}
owners = defaultdict(list)
missing = []
for name, crate_version in sorted(names):
    package = packages[(name, crate_version)]
    path = Path(package['manifest_path']).parent
    label = f'{name} {crate_version}'
    if package['source'] is None:
        # Local crates are covered by the repository's Apache-2.0 LICENSE,
        # except for any explicit crate licence included below.
        if not package['license']:
            raise SystemExit(f'local crate has no licence declaration: {label}')
        # Third-party crates vendored under vendor/ (rustrtc) keep their own
        # licence files, which ship with them.
        if path.is_relative_to(ROOT / 'vendor'):
            for source in files_at(path):
                add_text(texts, owners, source, label)
        continue
    found = files_at(path)
    found += notices_below(path)
    if name == 'openssl-src':
        found += files_at(path / 'openssl')
    if found:
        for source in found:
            add_text(texts, owners, source, label)
    else:
        missing.append((label, package['license']))

# Some published crates omit the licence file despite declaring an SPDX
# expression. Supply full shared texts and identify this limitation in the
# inventory. For dual licences Apache-2.0 is the selected alternative.
templates = {
    'Apache-2.0': ROOT / 'LICENSE',
    'MIT': None,
    'BSD-3-Clause': None,
}
for choice in ('MIT', 'BSD-3-Clause'):
    for package in packages.values():
        if package['source'] and package['license'] == choice:
            found = files_at(Path(package['manifest_path']).parent)
            if found:
                templates[choice] = found[0]
                break
for label, expression in missing:
    choice = ('BSD-3-Clause' if 'BSD-3-Clause' in expression else
              'Apache-2.0' if 'Apache-2.0' in expression else 'MIT')
    template = templates[choice]
    if template is None:
        raise SystemExit(f'no full licence template for {label} ({expression})')
    add_text(texts, owners, template, f'{label} [published crate omits text; {choice} selected]')

# Rollup lists only modules retained in output chunks. CSS carries the four
# self-hosted font files, whose original OFL texts are in the design system.
manifest = ROOT / 'ui/dashboard/dist/licenses/bundle-packages.json'
if not manifest.is_file():
    raise SystemExit('dashboard bundle package manifest missing; rebuild dashboard')
web_names = json.loads(manifest.read_text())
web = []
for name in web_names:
    path = npm_package_dir(name)
    info = json.loads((path / 'package.json').read_text())
    web.append((name, info['version'], info.get('license', 'NOT DECLARED')))
    found = files_at(path)
    if not found:
        raise SystemExit(f'bundled npm package has no licence text: {name}')
    for source in found:
        add_text(texts, owners, source, f'{name} {info["version"]} (dashboard)')
for name, file in (('Inter', 'LICENSE-Inter.txt'),
                   ('JetBrains Mono', 'LICENSE-JetBrainsMono.txt')):
    add_text(texts, owners, ROOT / 'ui/design-system/src/fonts' / file,
             f'{name} 5.3.0 (dashboard font, OFL-1.1)')

# The exact crates.io archive is the unmodified corresponding MPL source.
with (ROOT / 'Cargo.lock').open('rb') as source:
    locked = tomllib.load(source)['package']
checksum = next(p['checksum'] for p in locked if p['name'] == 'option-ext' and
                p['version'] == '0.2.0')
registry = Path(packages[('option-ext', '0.2.0')]['manifest_path']).parent
archive = registry.parents[2] / 'cache' / registry.parent.name / 'option-ext-0.2.0.crate'
if not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != checksum:
    raise SystemExit('option-ext crates.io source archive absent or checksum mismatch')
SOURCE.parent.mkdir(exist_ok=True)
shutil.copyfile(archive, SOURCE)

lines = [f'THIRD_PARTY_NOTICES for mina {version}', '',
         'Mina Rust is Apache-2.0; see LICENSE. This inventory covers the normal',
         'dependency graph for both Linux release targets with vendored OpenSSL,',
         'plus linked OpenSSL source and code/assets retained in the dashboard.',
         'THIRD_PARTY_LICENSES contains the full upstream texts and NOTICE files.',
         'Identical text bytes appear once, with every owner named above them.',
         '', 'Embedded Mina circuits, verifier indexes and genesis data derive',
         'from Mina OCaml 4.0.2 (Apache-2.0, Copyright 2024 Mina Foundation).',
         'The upstream revision 84b8d969 has no root NOTICE file.',
         '', 'option-ext 0.2.0 (MPL-2.0): its unmodified crates.io source is',
         f'SOURCES/option-ext-0.2.0.crate (SHA-256 {checksum}).',
         'The same archive is available from crates.io/api/v1/crates/',
         'option-ext/0.2.0/download. Its LICENSE.txt is included in the bundle.',
         '', 'Rust binary dependencies (name version | declared licence):']
for name, crate_version in sorted(names):
    package = packages[(name, crate_version)]
    lines.append(f'{name} {crate_version} | {package.get("license") or "NOT DECLARED"}')
lines += ['', 'Dashboard bundled packages (Rollup retained modules):']
for name, pkg_version, licence in web:
    lines.append(f'{name} {pkg_version} | {licence}')
lines += ['Inter 5.3.0 | OFL-1.1 (four self-hosted variable font assets)',
          'JetBrains Mono 5.3.0 | OFL-1.1',
          'Lucide SVG icons are generated by lucide-react above (ISC).',
          'The dashboard favicon.svg is original Mina Rust artwork.',
          '', 'Published Rust crates with a declared licence but no licence',
          'file in their registry source; the shared SPDX text is supplied:', *
          (f'{label} | {expr}' for label, expr in missing), '']
(ROOT / 'THIRD_PARTY_NOTICES').write_text('\n'.join(lines))
with BUNDLE.open('wb') as out:
    out.write(f'THIRD_PARTY_LICENSES for mina {version}\n'.encode())
    out.write(b'Exact upstream files, deduplicated by SHA-256. The header before\n')
    out.write(b'each text identifies every package using its exact bytes.\n\n')
    for digest, data in sorted(texts.items(), key=lambda item: (owners[item[0]][0], item[0])):
        out.write(('=' * 78 + '\n' + '\n'.join(sorted(owners[digest])) +
                   f'\nSHA-256: {digest}\n' + '=' * 78 + '\n').encode())
        out.write(data)
        if not data.endswith(b'\n'):
            out.write(b'\n')
        out.write(b'\n')
print(f'wrote notices ({len(names)} Rust, {len(web)} npm), '
      f'{len(texts)} distinct texts, {len(missing)} absent source texts, and MPL source')
