#!/usr/bin/env bash
# Builds the R8 benchmarks (native, WASM, WASM+SIMD) and runs them natively,
# in Node and in headless Chrome, writing one file per run to $OUT.
#
#   tools/wasm-poseidon-bench/scripts/run-all.sh [runs]
#
# Needs a static server for web/ on $PORT, e.g.
#   python3 -m http.server 4215 --bind 127.0.0.1 --directory web
set -euo pipefail

cd "$(dirname "$0")/.."
RUNS=${1:-3}
PORT=${PORT:-4215}
OUT=${OUT:-/tmp/r8/runs}
NIGHTLY=${NIGHTLY:-nightly-2026-07-15}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# chrome-headless-shell and its libraries, from a user-owned installation.
export LD_LIBRARY_PATH=${CHROME_LIBS:-$HOME/.local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu}
export CHROME=${CHROME:-$HOME/.local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell}
mkdir -p "$OUT"

cargo build --release
# RUSTFLAGS overrides the repo's threaded-WASM flags in .cargo/config.toml.
RUSTFLAGS="" cargo +"$NIGHTLY" build --release --lib --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/wasm_poseidon_bench.wasm web/plain.wasm
RUSTFLAGS="-C target-feature=+simd128" cargo +"$NIGHTLY" build --release --lib \
  --target wasm32-unknown-unknown --target-dir target-simd
cp target-simd/wasm32-unknown-unknown/release/wasm_poseidon_bench.wasm web/simd.wasm

for i in $(seq 1 "$RUNS"); do
  { uptime; ./target/release/native-bench; } > "$OUT/native-$i.txt" 2>&1
  { uptime; R8_JS_FLAGS=--no-liftoff node web/run-chrome.mjs "http://127.0.0.1:$PORT" plain.wasm,simd.wasm; } \
    > "$OUT/chrome-noliftoff-$i.txt" 2>&1
done
{ uptime; node web/run-chrome.mjs "http://127.0.0.1:$PORT" plain.wasm; } > "$OUT/chrome-default.txt" 2>&1
{ uptime; node --no-liftoff web/run-node.mjs web/plain.wasm; } > "$OUT/node-noliftoff.txt" 2>&1
echo "results in $OUT"
