#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
main=$(dirname "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)")
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target}
export RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals \
-C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--export-memory \
-C link-arg=--max-memory=4294967296 \
-C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
-C link-arg=--export=__tls_align -C link-arg=--export=__tls_base \
-C link-arg=--export=__heap_base"
cd "$repo"
"$main/tools/regression/lock.sh" build -- \
  cargo +nightly-2026-07-15 build --release --lib -p mina-isolated-prover \
    --target wasm32-unknown-unknown -Z build-std=panic_abort,std
wasm-bindgen --target web --out-dir "$repo/tools/isolated-prover/www/pkg" \
  "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/mina_isolated_prover.wasm"
