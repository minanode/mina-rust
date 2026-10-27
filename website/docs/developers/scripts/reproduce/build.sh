#!/usr/bin/env bash
set -euo pipefail
# Run from the checkout containing this reproduction pack.
rustup toolchain install 1.92 --profile minimal
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR="$PWD/target"
tools/regression/lock.sh build -- \
    cargo build --locked --release -p mina-cli --bin mina
(cd genesis_ledgers && sha256sum --check mesa-mainnet-without-ledgers.json.sha256)
target/release/mina node status --help
