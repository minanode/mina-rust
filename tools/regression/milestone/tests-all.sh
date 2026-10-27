#!/usr/bin/env bash
# Run after build-tests.sh and a native CLI build. Every mode gets fresh logs.
set -euo pipefail
s=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
: "${GATE_DIR:?set a fresh absolute gate output directory}"
: "${INPUT_DIR:?set the fixture directory documented in ../README.md}"
: "${MINA_BIN:?set the built native CLI}"
python3 -I "$s/pin-executables.py" test-build.jsonl scenarios-build.jsonl proof-example-build.jsonl binprot-build.jsonl
for mode in producer ordinary r26 i5c pool persist i5b writers example binprot \
  scenario-p2p_basic_incoming scenario-p2p_basic_connections scenario-p2p_kad scenario-p2p_basic_outgoing \
  ignored corpus; do
  python3 -I "$s/run-tests.py" "$mode" >"$GATE_DIR/tests/$mode-run.log" 2>&1
done
bash "$s/prover-tests.sh"
python3 -I "$s/summarize-tests.py" --json "$GATE_DIR/tests/results-all.json" | tee "$GATE_DIR/tests/summary.txt"
