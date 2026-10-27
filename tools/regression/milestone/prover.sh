#!/usr/bin/env bash
# Isolated browser production gate g; explicitly opt in by invoking this file.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
: "${GATE_DIR:?set a fresh absolute output directory}"
: "${CHROME:?install Chrome and set CHROME}"
: "${PKG:?set the freshly built wasm-bindgen --target web output directory}"
: "${VERIFIER:?set the built verify-isolated-proof binary}"
: "${MINA_BIN:?set the built mina CLI}"
: "${LEDGER_TOOL:?set the built ledger-tool}"
export MINA_BIN LEDGER_TOOL CHROME PKG VERIFIER
export PUPPETEER_DIR=${PUPPETEER_DIR:-$repo/crates/web-verifier/tests/browser}
export RUN=$GATE_DIR/prover/run OUT=$GATE_DIR/prover/out
export FAULT=0 BLOCKS=${1:-6}
[[ ! -e $RUN ]] || { echo 'use a fresh GATE_DIR' >&2; exit 2; }
mkdir -p "$GATE_DIR/prover"
trap 'rm -rf "$RUN/keys"' EXIT
cd "$repo"
"$repo/tools/regression/lock.sh" browser -- tools/webnode-block-prover/netns.sh bash -c '
  python3 -I tools/webnode-block-prover/prepare.py --dir "$RUN" \
    --mina "$MINA_BIN" --ledger-tool "$LEDGER_TOOL" --genesis-delay 360 &&
  node tools/webnode-block-prover/run.mjs'
