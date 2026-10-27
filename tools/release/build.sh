#!/usr/bin/env bash
# Build both unpublished Mesa devnet archives from this checkout.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo"
usage() { echo 'usage: build.sh VERSION [--allow-dirty]' >&2; exit 2; }
[[ $# -ge 1 && $# -le 2 ]] || usage
version=$1
public_repository_url=${RELEASE_REPOSITORY_URL:-https://github.com/minanode/mina-rust}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || usage
[[ $# -eq 1 || $2 == --allow-dirty ]] || usage
if [[ $# -eq 1 && -n $(git status --porcelain --untracked-files=normal) ]]; then
  echo 'release build requires a clean checkout; use --allow-dirty for a trial' >&2
  exit 2
fi
dirty=false
[[ -z $(git status --porcelain --untracked-files=normal) ]] || dirty=true
commit=$(git rev-parse HEAD)
epoch=$(git show -s --format=%ct HEAD)
build_date=$(date -u -d "@$epoch" +%FT%TZ)
rustc_version=$(rustc --version)
crate_version=$(python3 - <<'PY'
import tomllib
with open('Cargo.toml', 'rb') as stream:
    print(tomllib.load(stream)['workspace']['package']['version'])
PY
)
[[ $version == "$crate_version" ]] || {
  echo "archive version $version differs from crate version $crate_version" >&2
  exit 2
}
target_dir=$(realpath -m "${RELEASE_TARGET_DIR:-$repo/target/release-d6}")
mkdir -p "$repo/dist"
# F-console: the node dashboard, built first so that cargo embeds it in both
# binaries (ui/dashboard/dist; no npm runs at cargo time).
command -v npm >/dev/null || { echo 'npm required: the dashboard is built first' >&2; exit 2; }
node_version=$(node --version)
[[ $node_version == "v$(cat ui/.nvmrc)" ]] || { echo "Node $(cat ui/.nvmrc) required (ui/.nvmrc), found $node_version" >&2; exit 2; }
(cd ui && npm ci --no-audit --no-fund && npm run build:dashboard)
[[ -f ui/dashboard/dist/index.html ]] || { echo 'dashboard build missing' >&2; exit 1; }
python3 tools/release/generate-notices.py --dashboard-built
if [[ $# -eq 1 && -n $(git status --porcelain -- THIRD_PARTY_NOTICES \
    THIRD_PARTY_LICENSES SOURCES/option-ext-0.2.0.crate) ]]; then
  echo 'regenerated licence material differs from the checkout' >&2
  exit 2
fi
dashboard_sha256=$(cd ui/dashboard/dist && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1)
echo "dashboard built: dist sha256 $dashboard_sha256"
export CARGO_TARGET_DIR=$target_dir CARGO_BUILD_JOBS=8 SOURCE_DATE_EPOCH=$epoch
export PATH="$HOME/.local/opt/d5-cross/zig-linux-x86_64-0.13.0:$HOME/.local/bin:$PATH"
cargo_home=$(realpath "${CARGO_HOME:-$HOME/.cargo}")
rustup_sysroot=$(rustc --print sysroot)
# Keep source, registry, toolchain and compiler-wrapper paths out of panic text,
# __FILE__, and any native debug/file strings. The HOME rule is a fallback for
# cargo-zigbuild wrappers under ~/.cache. rustc and the C compilers apply the
# last matching map, so HOME comes first and the specific roots after it.
rust_remaps="--remap-path-prefix=$HOME=/build-home --remap-path-prefix=$repo=/build/mina-rust --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$rustup_sysroot=/rustup --remap-path-prefix=$target_dir=/build-target"
c_remaps="-ffile-prefix-map=$HOME=/build-home -ffile-prefix-map=$repo=/build/mina-rust -ffile-prefix-map=$cargo_home=/cargo -ffile-prefix-map=$rustup_sysroot=/rustup -ffile-prefix-map=$target_dir=/build-target"
recorded_remaps='--remap-path-prefix=<home>=/build-home --remap-path-prefix=<repo>=/build/mina-rust --remap-path-prefix=<cargo-home>=/cargo --remap-path-prefix=<rustup-sysroot>=/rustup --remap-path-prefix=<target-dir>=/build-target'
recorded_cflags='-ffile-prefix-map=<home>=/build-home -ffile-prefix-map=<repo>=/build/mina-rust -ffile-prefix-map=<cargo-home>=/cargo -ffile-prefix-map=<rustup-sysroot>=/rustup -ffile-prefix-map=<target-dir>=/build-target'
export CFLAGS="$c_remaps" CXXFLAGS="$c_remaps"
command -v cargo-zigbuild >/dev/null
command -v zig >/dev/null
command -v strings >/dev/null
[[ $(zig version) == 0.13.0 ]] || { echo 'Zig 0.13.0 required' >&2; exit 2; }
[[ $(cargo-zigbuild --version) == *0.20.0* ]] || { echo 'cargo-zigbuild 0.20.0 required' >&2; exit 2; }
# MEM-ARM: the allocator check runs the ARM64 binary under qemu-user, with an
# ARM64 glibc loader and libraries (Ubuntu: qemu-user-static and
# libc6-arm64-cross, or the same packages extracted under ~/.local/opt).
export PATH="$HOME/.local/opt/qemu-user-static/usr/bin:$PATH"
export QEMU_AARCH64=${QEMU_AARCH64:-qemu-aarch64-static}
if [[ -z ${ARM64_SYSROOT:-} ]]; then
  ARM64_SYSROOT=/usr/aarch64-linux-gnu
  [[ -e $HOME/.local/opt/arm64-sysroot/lib/ld-linux-aarch64.so.1 ]] && ARM64_SYSROOT=$HOME/.local/opt/arm64-sysroot
fi
export ARM64_SYSROOT
command -v "$QEMU_AARCH64" >/dev/null || { echo 'qemu-aarch64-static required for the ARM64 allocator check' >&2; exit 2; }
[[ -e $ARM64_SYSROOT/lib/ld-linux-aarch64.so.1 ]] || { echo 'ARM64 glibc sysroot required (ARM64_SYSROOT)' >&2; exit 2; }
for arch in x86_64 aarch64; do
  if [[ $arch == x86_64 ]]; then
    target=x86_64-unknown-linux-gnu
    label=linux-x86_64
    cpu_note='Requires BMI2 and ADX (Intel Broadwell+ / AMD Zen 1+). Older CPUs may SIGILL.'
    export RUSTFLAGS="-C target-feature=+bmi2,+adx $rust_remaps"
    recorded_rustflags="-C target-feature=+bmi2,+adx $recorded_remaps"
  else
    target=aarch64-unknown-linux-gnu
    label=linux-arm64
    cpu_note='Requires AArch64 ARMv8-A Linux with 4, 16 or 64 KiB pages.'
    export RUSTFLAGS="$rust_remaps"
    recorded_rustflags=$recorded_remaps
  fi
  floor=${RELEASE_GLIBC_FLOOR:-2.31}
  [[ $floor =~ ^[0-9]+\.[0-9]+$ ]] || { echo 'invalid glibc floor' >&2; exit 2; }
  free=$(df -BG --output=avail /home | tail -1 | tr -dc '0-9')
  (( free >= 55 )) || { echo "only ${free} GiB free; need 55 GiB" >&2; exit 2; }
  echo "building $label for glibc $floor ($free GiB free)"
  "$repo/tools/regression/lock.sh" build -- nice -n 10 cargo zigbuild \
    --locked --release -p mina-cli --bin mina --features vendored-openssl \
    --target "${target}.${floor}"
  binary="$target_dir/$target/release/mina"
  [[ -x $binary ]] || { echo "missing $binary" >&2; exit 1; }
  # OpenSSL embeds its install prefix and compiler command as literal strings,
  # and env!(CARGO_MANIFEST_DIR) embeds a source path. Prefix maps do not alter
  # these constants. Replace those exact roots with equal-length neutral paths
  # before the guard; byte length and ELF layout stay unchanged.
  python3 - "$binary" "$repo" "$HOME" "$target_dir" <<'PY'
from pathlib import Path
import sys
binary, repo, home, target = map(Path, sys.argv[1:])
data = binary.read_bytes()
# An external shared target is not beneath repo. Replace it before the broader
# home prefix so its cache-directory suffix cannot survive a partial scrub.
for old, new in ((str(target).encode(), b'/build-target'),
                 (str(repo).encode(), b'/build/mina-rust'),
                 (str(home).encode(), b'/build-home')):
    if len(new) > len(old):
        raise SystemExit('replacement path exceeds source path length')
    padded = new + b'_' * (len(old) - len(new))
    count = data.count(old)
    data = data.replace(old, padded)
    print(f'scrubbed {count} embedded literal path(s)')
binary.write_bytes(data)
PY
  python3 - "$binary" "$repo" "$(id -un)" "$label" <<'PY'
import re, subprocess, sys
binary, repo, username, label = sys.argv[1:]
# The worktree layout (.t3/worktrees/...) must not survive a partial remap.
patterns = ('/home/', username, repo, '.t3/', 'worktrees/')
counts = {pattern: 0 for pattern in patterns}
unexpected_user = 0
with subprocess.Popen(['strings', '-a', binary], stdout=subprocess.PIPE,
                      text=True, errors='replace') as process:
    for line in process.stdout:
        for pattern in patterns:
            counts[pattern] += pattern in line
        # "minaboros" is the consensus protocol name, not builder identity.
        unexpected_user += bool(re.search(r'(?<!mina)' + re.escape(username), line))
    if process.wait():
        raise SystemExit('strings failed')
print(f'path scan {label}: ' + ', '.join(f'{key}={count}' for key, count in counts.items()))
if (counts['/home/'] or counts[repo] or counts['.t3/'] or counts['worktrees/']
        or unexpected_user):
    raise SystemExit('release binary contains a local path or username')
PY
  stage=$(mktemp -d "$repo/dist/.stage.XXXXXXXX")
  cp "$binary" "$stage/mina"
  # Check the exact executable shipped in the archive, after path scrubbing.
  # The Zig release output can already be stripped, so runtime checks are
  # mandatory even when the structural symbol check is unavailable.
  # Both targets use jemalloc; ARM64 also checks its 64 KiB page build on
  # emulated 16 and 64 KiB kernels.
  "$repo/tools/regression/check-jemalloc-defaults.sh" "$stage/mina"
  "$repo/tools/release/stage-licenses.sh" "$stage"
  cat >"$stage/README.txt" <<EOF
Mina Rust ${version} (${label}) - experimental Mesa devnet only.
The archive and mina --version both report ${crate_version}.
Quick start: https://minanode.org/docs/quickstart/
The default devnet genesis config is embedded; no separate genesis file is needed.
Minimum to try: full node 1 core/2 GiB RAM/5 GB disk; producer or
SNARK worker 4 cores/8 GiB RAM/5 GB disk.
Use stable broadband (10 Mbps suggested) and an NTP synchronized clock.
$cpu_note
See https://minanode.org/docs/requirements/ for limits and recommended headroom.

Verify

Use OpenSSH 8.2 or newer. Download SHA256SUMS and SHA256SUMS.sig from the
same GitHub release. Download the release signer from minanode.org:
  url=${public_repository_url}/releases/download/v${version}
  curl --fail-early -fLO "\$url/SHA256SUMS" \\
    -O "\$url/SHA256SUMS.sig" -O https://minanode.org/allowed_signers
  ssh-keygen -lf allowed_signers -E sha256
Compare that fingerprint with https://minanode.org/docs/install/#check and
confirm it through a separate trusted channel for a new or changed key.
  ssh-keygen -Y verify -f allowed_signers -I release@minanode.org \\
    -n file -s SHA256SUMS.sig < SHA256SUMS &&
  sha256sum --check --ignore-missing SHA256SUMS
Continue only after a good signature and an OK checksum, before unpacking.
Run ./mina --help and ./mina build-info.

Run a devnet node: ./mina node --network devnet
It runs in the foreground and stops when you close the terminal. To keep it
running, see https://minanode.org/docs/run-a-node/#service
EOF
  BUILD_COMMIT=$commit BUILD_VERSION=$version BUILD_TARGET=$label \
    BUILD_GLIBC=$floor BUILD_RUSTC="$rustc_version" BUILD_DATE=$build_date \
    BUILD_RUSTFLAGS=$recorded_rustflags BUILD_CFLAGS=$recorded_cflags \
    BUILD_DIRTY=$dirty BUILD_DASHBOARD_SHA256=$dashboard_sha256 \
    BUILD_NODE_VERSION=$node_version \
    python3 - "$stage/build-info.json" <<'PY'
import json, os, sys
keys = {'commit':'BUILD_COMMIT', 'version':'BUILD_VERSION',
        'target':'BUILD_TARGET', 'glibc_floor':'BUILD_GLIBC',
        'rustc_version':'BUILD_RUSTC', 'build_date':'BUILD_DATE',
        'RUSTFLAGS':'BUILD_RUSTFLAGS'}
data = {key: os.environ[value] for key, value in keys.items()}
data['features'] = ['embedded-circuits', 'p2p-libp2p', 'p2p-webrtc',
                    'exit-on-invalid-peers', 'dashboard']
# p2p-webrtc builds the rustrtc backend (p2p-webrtc-rs selects webrtc-rs).
data['webrtc_backend'] = 'rustrtc'
# The embedded dashboard: the SHA-256 of its files' sha256sum list, sorted
# by path, in ui/dashboard/dist, and the Node.js that built it.
data['dashboard'] = {'dist_sha256': os.environ['BUILD_DASHBOARD_SHA256'],
                     'node_version': os.environ['BUILD_NODE_VERSION']}
data['dirty_checkout'] = os.environ['BUILD_DIRTY'] == 'true'
data['CFLAGS'] = os.environ['BUILD_CFLAGS']
data['CXXFLAGS'] = os.environ['BUILD_CFLAGS']
with open(sys.argv[1], 'w') as out:
    json.dump(data, out, indent=2, sort_keys=True)
    out.write('\n')
PY
  tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
    -C "$stage" -cf - LICENSE THIRD_PARTY_NOTICES THIRD_PARTY_LICENSES \
      SOURCES README.txt build-info.json mina | \
    gzip -n -9 >"dist/mina-${version}-${label}.tar.gz"
  rm -rf "$stage"
done
(cd dist && sha256sum "mina-${version}-linux-x86_64.tar.gz" \
  "mina-${version}-linux-arm64.tar.gz" >SHA256SUMS)
echo 'archives and SHA256SUMS are in dist/'
