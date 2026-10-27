#!/usr/bin/env bash
# Smoke an unpublished release archive. Keep logs under .work/D6.
set -euo pipefail
[[ $# -eq 1 ]] || { echo 'usage: smoke.sh TARBALL' >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
archive=$(realpath "$1")
name=$(basename "$archive")
[[ $name == mina-*-linux-*.tar.gz ]] || { echo 'unexpected archive name' >&2; exit 2; }
run="$repo/.work/D6/smoke-${name%.tar.gz}-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$run/unpacked" "$run/home" "$run/tmp" "$run/node"
tar -xzf "$archive" -C "$run/unpacked" --no-same-owner
binary="$run/unpacked/mina"
[[ -x $binary ]] || { echo 'archive has no executable mina' >&2; exit 1; }
file "$binary" | tee "$run/elf.log"
readelf -h "$binary" | grep Machine | tee -a "$run/elf.log"
readelf -l "$binary" | grep 'Requesting program interpreter' | tee -a "$run/elf.log"
objdump -T "$binary" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu | tail -1 | tee -a "$run/elf.log"
python3 - "$run/unpacked/build-info.json" "$run/elf.log" "$name" "$run/unpacked/README.txt" <<'PY'
import json, re, sys
meta = json.load(open(sys.argv[1]))
elf = open(sys.argv[2]).read()
archive = sys.argv[3]
readme = open(sys.argv[4]).read()
assert archive == f"mina-{meta['version']}-{meta['target']}.tar.gz"
assert 'devnet only' in readme
actual = re.findall(r'GLIBC_(\d+\.\d+)', elf)[-1]
expected = {
    'linux-x86_64': ('Advanced Micro Devices X86-64', '/lib64/ld-linux-x86-64.so.2'),
    'linux-arm64': ('AArch64', '/lib/ld-linux-aarch64.so.1'),
}[meta['target']]
assert all(value in elf for value in expected), (meta['target'], expected)
assert tuple(map(int, actual.split('.'))) <= tuple(map(int, meta['glibc_floor'].split('.'))), (actual, meta['glibc_floor'])
assert 'embedded-circuits' in meta['features']
assert meta['webrtc_backend'] == 'rustrtc', meta.get('webrtc_backend')
print('ELF glibc ceiling:', actual, 'declared floor:', meta['glibc_floor'])
PY
case "$name:$(uname -m)" in
  *linux-x86_64.tar.gz:x86_64|*linux-arm64.tar.gz:aarch64) ;;
  *) echo "cross-architecture ELF check complete; runtime needs matching Linux host"; exit 0 ;;
esac
export HOME="$run/home" TMPDIR="$run/tmp"
"$binary" --version | tee "$run/version.log"
python3 - "$run/version.log" "$run/unpacked/build-info.json" <<'PY'
import json, sys
meta = json.load(open(sys.argv[2]))
expected = f"mina {meta['version']} (WebRTC: {meta['webrtc_backend']})"
assert open(sys.argv[1]).read().strip() == expected, expected
PY
"$binary" build-info | tee "$run/build-info.log"
cd "$repo"
tools/regression/check-build-env.sh "$binary" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$run/unpacked/build-info.json")" | tee "$run/commit.log"
for port in 3705 8875; do
  if ss -ltn "( sport = :$port )" | tail -n +2 | grep -q .; then
    echo "port $port already in use" >&2
    exit 2
  fi
done
cleanup() {
  if [[ -n ${pid:-} ]] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid"
    wait "$pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT
"$binary" --network devnet node -d "$run/node" -p 3705 \
  --libp2p-port 8875 \
  --disable-filesystem-logging >"$run/node.log" 2>&1 &
pid=$!
deadline=$((SECONDS + 600))
while (( SECONDS < deadline )); do
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "node exited before sync; see $run/node.log" >&2
    exit 1
  fi
  if curl -fsS --max-time 3 http://127.0.0.1:3705/status >"$run/status.json" 2>/dev/null; then
    if python3 - "$run/status.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
s = (d.get('transition_frontier') or {}).get('sync') or {}
tip = (d.get('transition_frontier') or {}).get('best_tip') or {}
peers = [p.get('best_tip_height') for p in d.get('peers') or []
         if p.get('connection_status') == 'Connected' and p.get('best_tip_height') is not None]
if s.get('status') == 'Synced' and peers and tip.get('height') is not None and max(peers) - tip['height'] <= 2:
    print('Synced: local height', tip['height'], 'peer max', max(peers))
else:
    sys.exit(1)
PY
    then
      echo "runtime smoke passed; logs: $run"
      exit 0
    fi
  fi
  sleep 5
done
echo "timed out waiting for Synced near peer tip; logs: $run" >&2
exit 1
