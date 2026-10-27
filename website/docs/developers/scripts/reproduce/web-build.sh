#!/usr/bin/env bash
set -euo pipefail
# Run install-rust.sh and install-wasm-tools.sh first (see imported setup scripts).
: "${RELAY:?set your reachable relay WebRTC multiaddr}"
export NETWORK=${NETWORK:-devnet}
export RELAY CARGO_BUILD_JOBS=8
OUT=${OUT:-$PWD/webnode-build}
tools/regression/lock.sh build -- tools/regression/webnode/build.sh "$OUT"
cd "$OUT/browser"
# Use your assigned static port on the development machine.
npx -y serve@14 -s . -l "${STATIC_PORT:-4211}" -c serve.json
