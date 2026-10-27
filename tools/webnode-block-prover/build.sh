#!/usr/bin/env bash
# Build the web node (mina-node-web) for the I34c harness, under the shared
# build lock, and run wasm-bindgen into $PKG. The recipe is the one in
# Build recipe: nightly-2026-07-15, the crate's build-std and
# threads config, and the __heap_base export; wasm-bindgen must match
# Cargo.lock (0.2.106).
#
#   CARGO_TARGET_DIR=/tmp/i34c/target-wasm PKG=/tmp/i34c/pkg \
#     tools/webnode-block-prover/build.sh
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
main=$(dirname "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)")
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target}
pkg=${PKG:?set PKG to the wasm-bindgen output directory}
cd "$repo/crates/node/web"
"$main/tools/regression/lock.sh" build -- \
  cargo +nightly-2026-07-15 build --release --target wasm32-unknown-unknown \
    --config 'target.wasm32-unknown-unknown.rustflags=["-C","link-arg=--export=__heap_base"]'
wasm-bindgen --target web --out-dir "$pkg" \
  "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/mina_node_web.wasm"
sha256sum "$pkg/mina_node_web_bg.wasm"
