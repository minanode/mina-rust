#!/usr/bin/env bash
# Build the threaded wasm verifier and stage the demo page in crates/web-verifier/www.
#
# Needs the pinned nightly with rust-src and the wasm32 target, plus a
# wasm-bindgen-cli matching the workspace's wasm-bindgen version:
#   rustup toolchain install nightly-2026-07-15 -c rust-src -t wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.106 --locked
set -euo pipefail

nightly="${NIGHTLY:-nightly-2026-07-15}"
crate_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$crate_dir/../.." && pwd)"
www="$crate_dir/www"

# RUSTFLAGS replaces the cargo config's wasm flags entirely, so all threading
# flags are listed here. `__heap_base` must be exported for wasm-bindgen's
# thread transform.
export RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals \
  -C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--export-memory \
  -C link-arg=--max-memory=4294967296 \
  -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
  -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base \
  -C link-arg=--export=__heap_base"

cd "$root"
cargo "+$nightly" build --release -p mina-web-verifier \
  --target wasm32-unknown-unknown -Z build-std=panic_abort,std

wasm-bindgen --target web --out-dir "$www/pkg" \
  target/wasm32-unknown-unknown/release/mina_web_verifier.wasm

mkdir -p "$www/vk"
cp "$root"/data/vk/*-mesa-blockchain-vk.json "$www/vk/"

ls -l "$www/pkg"/*.wasm
