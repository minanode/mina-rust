#!/usr/bin/env bash
set -euo pipefail
export CARGO_BUILD_JOBS=8
tools/regression/lock.sh build -- make test-release
# For the selected m9 gate, build and pin exact executables instead:
# export GATE_DIR="$PWD/gate-results" INPUT_DIR="$PWD/reproduction-inputs"
# export MINA_BIN="$PWD/target/release/mina"
# export MINA_CIRCUIT_BLOBS_BASE_DIR="$INPUT_DIR/circuit-blobs"
# bash tools/regression/milestone/build-tests.sh
# bash tools/regression/milestone/tests-all.sh
