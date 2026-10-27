#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

# WASM builds use this nightly's rust-src and target, not the stable target.
rustup toolchain install nightly-2026-07-15 --profile minimal
rustup component add rust-src --toolchain nightly-2026-07-15
rustup target add wasm32-unknown-unknown --toolchain nightly-2026-07-15

# Run under tools/regression/lock.sh build with CARGO_BUILD_JOBS=8.
cargo install wasm-pack --locked
cargo install wasm-bindgen-cli --version 0.2.106 --locked
