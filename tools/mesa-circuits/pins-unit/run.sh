#!/usr/bin/env bash
# Run under the repository build lock, as shown in README.md.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../../.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target/pins-unit}
run_native() {
    cargo +nightly-2026-07-15 test --locked --manifest-path "$here/Cargo.toml"
}
run_wasm() {
    CARGO_ENCODED_RUSTFLAGS= cargo +nightly-2026-07-15 test --locked \
        --manifest-path "$here/Cargo.toml" --target wasm32-unknown-unknown \
        --config 'target.wasm32-unknown-unknown.runner="wasm-bindgen-test-runner"'
}
case ${1:-both} in
    native) run_native ;;
    wasm) run_wasm ;;
    both) run_native; run_wasm ;;
    *) echo 'usage: run.sh [native|wasm|both]' >&2; exit 2 ;;
esac
