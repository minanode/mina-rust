#!/usr/bin/env bash
# Offline gate b. Pass --no-reconstruct if the archived rebuild dump is absent.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
: "${GATE_DIR:?set an absolute output directory}"
: "${INPUT_DIR:?set the fixture directory}"
: "${BIN_DIR:?set the directory containing mina-block-replay and profile_reconstruct}"
export RAYON_NUM_THREADS=${RAYON_NUM_THREADS:-8}
mkdir -p "$GATE_DIR/replay"
exec python3 "$repo/tools/regression/baseline.py" --work "$INPUT_DIR" \
  --bin-dir "$BIN_DIR" --replay full --log-dir "$GATE_DIR/replay" "$@"
