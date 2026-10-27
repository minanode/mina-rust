#!/usr/bin/env bash
# Runs a subset of kernels in headless Chrome (TurboFan only) several times.
#
#   tools/wasm-poseidon-bench/scripts/run-chrome-filter.sh <module> <regex> <tag> [runs]
set -euo pipefail

cd "$(dirname "$0")/.."
MODULE=$1
FILTER=$2
TAG=$3
RUNS=${4:-2}
PORT=${PORT:-4215}
OUT=${OUT:-/tmp/r8/runs2}
# chrome-headless-shell and its libraries, from a user-owned installation.
export LD_LIBRARY_PATH=${CHROME_LIBS:-$HOME/.local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu}
export CHROME=${CHROME:-$HOME/.local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell}
export R8_JS_FLAGS=${R8_JS_FLAGS:---no-liftoff}
mkdir -p "$OUT"
for i in $(seq 1 "$RUNS"); do
  { uptime; node web/run-chrome.mjs "http://127.0.0.1:$PORT" "$MODULE" "$FILTER"; } > "$OUT/$TAG-$i.txt" 2>&1
done
cat "$OUT/$TAG"-*.txt
