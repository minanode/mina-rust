#!/usr/bin/env bash
set -euo pipefail
export INPUT_DIR=${INPUT_DIR:-$PWD/reproduction-inputs}
export GATE_DIR=${GATE_DIR:-$PWD/gate-results}
export BIN_DIR="$PWD/target/release"
export CARGO_BUILD_JOBS=8 RAYON_NUM_THREADS=8
[[ -f $INPUT_DIR/genesis-mainnet/mesa-mainnet.bin ]] || {
    echo 'Supply the pinned binary (publication pending), or regenerate it first.' >&2
    exit 2
}
tools/regression/lock.sh build -- cargo build --locked --release \
    -p mina-block-replay --features profile --bin mina-block-replay \
    --example profile_reconstruct --example check_chain
mkdir -p "$INPUT_DIR/replay/blocks"
python3 tools/block-replay/fetch_chain.py --from-height 548148 --tip-height 564044 \
    --tip 3NL9ixEeoyrPa8nLyyKsRnHNuMg6yoaztdFdK5ZbRmJkPqqBE5Hd \
    --out "$INPUT_DIR/replay/blocks"
cp "$INPUT_DIR/replay/blocks/chain.txt" "$INPUT_DIR/replay/chain.txt"
tools/regression/inputs.py check --dir "$INPUT_DIR" tools/regression/inputs/replay-subset.txt
"$BIN_DIR/examples/check_chain" --chain "$INPUT_DIR/replay/chain.txt" \
    --blocks "$INPUT_DIR/replay/blocks"
bash tools/regression/milestone/full-replay.sh --no-reconstruct
