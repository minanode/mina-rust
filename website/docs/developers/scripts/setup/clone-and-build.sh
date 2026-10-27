#!/usr/bin/env bash
set -euo pipefail
git clone https://github.com/minanode/mina-rust.git
cd mina-rust

# A non-producing Mesa node uses embedded verification data.
# Prover circuits are a separate, currently unpublished prerequisite.
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
    cargo build --locked --release -p mina-cli --bin mina
