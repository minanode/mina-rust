#!/usr/bin/env bash
# Builds the benchmarks with the WebAssembly wide-arithmetic proposal enabled
# (i64.mul_wide_u, i64.add128: LLVM then lowers arkworks' u128 arithmetic to
# them) and runs them in headless Chrome with V8's experimental flag.
#
#   tools/wasm-poseidon-bench/scripts/run-wide.sh [runs]
#
# Needs the static server for web/ on $PORT, as for run-all.sh.
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

RUSTFLAGS="-C target-feature=+wide-arithmetic" cargo +"$NIGHTLY" build --release --lib \
  --target wasm32-unknown-unknown --target-dir target-wide
cp target-wide/wasm32-unknown-unknown/release/wasm_poseidon_bench.wasm web/wide.wasm

for i in $(seq 1 "$RUNS"); do
  { uptime; R8_JS_FLAGS="--no-liftoff --wasm-wide-arithmetic" \
      node web/run-chrome.mjs "http://127.0.0.1:$PORT" wide.wasm; } > "$OUT/chrome-wide-$i.txt" 2>&1
done
echo "results in $OUT"
