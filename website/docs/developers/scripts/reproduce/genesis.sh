#!/usr/bin/env bash
set -euo pipefail
# Requires installed OCaml 4.0.2 tooling; publication pending (see inventory).
: "${OCAML_MINA_PREFIX:?set the extracted OCaml 4.0.2 prefix}"
export INPUT_DIR=${INPUT_DIR:-$PWD/reproduction-inputs}
export CARGO_BUILD_JOBS=8
mkdir -p "$INPUT_DIR/configs"
curl -fsSL https://raw.githubusercontent.com/MinaProtocol/mina/84b8d96954b218f372b93f65a071abf528e6bb61/genesis_ledgers/mainnet.json \
    -o "$INPUT_DIR/configs/mainnet.json"
tools/regression/lock.sh build -- cargo build --locked --release -p ledger-tool
tools/ledger-tool/genesis-from-tarballs.sh "$INPUT_DIR/configs/mainnet.json" \
    "$INPUT_DIR/genesis-mainnet" mesa-mainnet
tools/regression/inputs.py check --dir "$INPUT_DIR" --genesis-roots \
    --ledger-tool "$PWD/target/release/ledger-tool" tools/regression/inputs/genesis.txt
