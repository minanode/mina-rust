#!/usr/bin/env python3
"""Regenerate release notices, exact upstream texts, and MPL source."""

import argparse
import hashlib
import io
import json
import os
import re
import subprocess
import tomllib
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def cargo(*args):
    # metadata and tree do not compile; do not acquire a nested build lock.
    return subprocess.check_output(
        ['cargo', *args],
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
parser.add_argument('--web-built', type=Path,
                    help='browser directory from the web release build')
parser.add_argument('--output', type=Path,
                    help='notice directory (default: repository root)')
parser.add_argument('--check', action='store_true',
                    help='fail if generated material differs; do not write')
args = parser.parse_args()
output = args.output or ROOT
app = 'web node' if args.web_built else 'dashboard'
if not args.dashboard_built and not args.web_built:
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
closure = []
if args.web_built:
    targets = ('wasm32-unknown-unknown',)
    build = json.loads((args.web_built / 'build-info.json').read_text())
    features = build['wasm']['features'] or ''
    selection = ['-p', 'mina-node-web']
    if features:
        selection += ['--features', features]
else:
    targets = ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu')
    selection = ['-p', 'mina-cli', '--features', 'vendored-openssl']
for target in targets:
    tree = cargo('tree', '--locked', *selection, '-e', 'normal',
                 '--target', target, '--prefix', 'none', '--format', '{p}|{f}')
    for line in tree.splitlines():
        match = re.match(r'^([^ ]+) v([^ |]+)', line)
        if match:
            names.add(match.groups())
            # Features are part of the audited closure, as well as versions.
            closure.append([*match.groups(), line.split('|')[1].removesuffix(' (*)')])

# openssl-src is a build dependency, but its OpenSSL source is linked in.
if not args.web_built:
    extra = ('openssl-src', '300.4.1+3.4.0')
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
manifest = (args.web_built or ROOT / 'ui/dashboard/dist') / 'licenses/bundle-packages.json'
if not manifest.is_file():
    raise SystemExit(f'{app} bundle package manifest missing; rebuild app')
web_names = json.loads(manifest.read_text())
web = []
for entry in web_names:
    if isinstance(entry, str):
        if args.web_built:
            raise SystemExit('web package inventory must identify exact installed versions')
        name = entry
        path = npm_package_dir(name)
    else:
        name = entry['name']
        path = (ROOT / 'ui' / entry['path']).resolve()
        if not path.is_relative_to(ROOT / 'ui/node_modules'):
            raise SystemExit(f'package outside installed dependencies: {name}')
    info = json.loads((path / 'package.json').read_text())
    if not isinstance(entry, str) and (info['name'], info['version']) != (name, entry['version']):
        raise SystemExit(f'installed package differs from retained module: {name}')
    web.append((name, info['version'], info.get('license', 'NOT DECLARED')))
    found = files_at(path)
    if not found:
        raise SystemExit(f'bundled npm package has no licence text: {name}')
    for source in found:
        add_text(texts, owners, source, f'{name} {info["version"]} ({app})')
for name, file in (('Inter', 'LICENSE-Inter.txt'),
                   ('JetBrains Mono', 'LICENSE-JetBrainsMono.txt')):
    add_text(texts, owners, ROOT / 'ui/design-system/src/fonts' / file,
             f'{name} 5.3.0 ({app} font, OFL-1.1)')

generated = {}
checksum = None
if ('option-ext', '0.2.0') in names:
    # The exact crates.io archive is the unmodified corresponding MPL source.
    with (ROOT / 'Cargo.lock').open('rb') as source:
        locked = tomllib.load(source)['package']
    checksum = next(p['checksum'] for p in locked if p['name'] == 'option-ext' and
                    p['version'] == '0.2.0')
    registry = Path(packages[('option-ext', '0.2.0')]['manifest_path']).parent
    archive = registry.parents[2] / 'cache' / registry.parent.name / 'option-ext-0.2.0.crate'
    if not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != checksum:
        raise SystemExit('option-ext crates.io source archive absent or checksum mismatch')
    generated['SOURCES/option-ext-0.2.0.crate'] = archive.read_bytes()
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
if args.web_built:
    lines[2:6] = [
        'Mina Rust is Apache-2.0; see LICENSE. This inventory covers the normal',
        'mina-node-web dependency graph for wasm32-unknown-unknown and the',
        'JavaScript modules retained by Rollup in the web node application.',
        'THIRD_PARTY_LICENSES contains the full upstream texts and NOTICE files.',
    ]
    if checksum is None:
        start = next(i for i, line in enumerate(lines) if line.startswith('option-ext 0.2.0 (MPL'))
        del lines[start:start + 5]
    lines = [line.replace('Rust binary dependencies', 'Rust WASM dependencies')
             .replace('Dashboard bundled packages', 'Web node bundled packages')
             .replace('dashboard favicon.svg', 'web node favicon.svg') for line in lines]
    inventory = {
        'target': targets[0], 'features': features,
        'rust': [{'name': name, 'version': version,
                  'source': packages[(name, version)]['source'],
                  'features': sorted(set(row[2] for row in closure if row[:2] == [name, version]))}
                 for name, version in sorted(names)],
        'javascript': web_names,
    }
    generated['closure.json'] = (json.dumps(inventory, indent=2) + '\n').encode()
generated['THIRD_PARTY_NOTICES'] = '\n'.join(lines).encode()
out = io.BytesIO()
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
generated['THIRD_PARTY_LICENSES'] = out.getvalue()
for name, data in generated.items():
    destination = output / name
    if args.check:
        if not destination.is_file() or destination.read_bytes() != data:
            raise SystemExit(f'stale release notice material: {name}; regenerate and commit it')
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
print(f'{"checked" if args.check else "wrote"} notices ({len(names)} Rust, {len(web)} npm), '
      f'{len(texts)} distinct texts, {len(missing)} absent source texts')
