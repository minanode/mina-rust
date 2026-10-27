#!/usr/bin/env bash
# Dump the Mesa circuit blobs that mina-tree's provers load (*_gates.json,
# *_internal_vars.bin, *_rows_rev.bin) with the official OCaml 4.0.2 binary
# (scripts/ocaml-mina). Offline: no node, no peers, no fixed ports.
#
#   tools/mesa-circuits/generate.sh <out-dir> [fork-config.json]
#   tools/mesa-circuits/generate.sh --check <out-dir>
#
# 1. `transaction-snark-profiler` compiles the transaction circuits (base,
#    merge, three zkApp step circuits, transaction wrap). It then fails while
#    proving, because it signs with the testnet signature kind, but only after
#    the keys and blobs are written. That failure is accepted only if all six
#    transaction circuits were dumped.
# 2. `internal test-genesis-block-generation` compiles the blockchain step and
#    wrap circuits with the constraint constants of the config file, which
#    carries the Mesa fork constants, and proves a genesis block in a local
#    prover child process.
# 3. Checks (also run alone with --check): exactly 8 circuits, each with its
#    three files (24 files), and the blockchain wrap verification key contains
#    every commitment coordinate of the live key in
#    data/vk/<network>-mesa-blockchain-vk.json (56/56); MESA_CIRCUITS_LIVE_KEY=none
#    skips that comparison for a fork other than the published one. The full check, which
#    also rebuilds every verifier index in Rust, is the mesa_circuit_blobs
#    example of mina-tree.
# 4. Links the 24 files into <out-dir>/circuit-blobs/4.0.2<network>/ (the
#    `directory_name` of the network's `CircuitsConfig` in
#    crates/core/src/network.rs; MINA_CIRCUITS_DIR overrides it), so that
#    MINA_CIRCUIT_BLOBS_BASE_DIR=<out-dir>/circuit-blobs gives the prover
#    the blobs. --check checks that directory too, when there is one.
#
# MINA_DUMP_CIRCUIT_DATA makes pickles dump the blobs next to each proving
# key it writes; only the legacy key format does this, hence
# MINA_USE_MMAP_CACHE=0. TMPDIR moves the key cache (coda_cache_dir) out of
# /tmp so no existing keys are reused. Set MINA_PROFILE=devnet and pass the
# devnet fork config for devnet.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
export MINA_PROFILE="${MINA_PROFILE:-mainnet}"
circuits_dir="${MINA_CIRCUITS_DIR:-4.0.2$MINA_PROFILE}"

fail() {
  echo "generate.sh: $*" >&2
  exit 1
}

count() {
  if [ -d "$1" ]; then
    find -L "$1" -maxdepth 1 -type f -name "$2" | wc -l
  else
    echo 0
  fi
}

check() {
  local cache=$1
  [ -d "$cache" ] || fail "no key cache directory $cache"
  local gates internal rows
  gates=$(count "$cache" '*_gates.json')
  internal=$(count "$cache" '*_internal_vars.bin')
  rows=$(count "$cache" '*_rows_rev.bin')
  if [ "$gates" -ne 8 ] || [ "$internal" -ne 8 ] || [ "$rows" -ne 8 ]; then
    fail "expected 8 circuits with 3 files each, found $gates gates, $internal internal_vars, $rows rows_rev files in $cache"
  fi
  local f
  for f in "$cache"/*_gates.json; do
    local base=${f%_gates.json}
    if [ ! -s "$base"_internal_vars.bin ] || [ ! -s "$base"_rows_rev.bin ]; then
      fail "incomplete circuit $(basename "$base")"
    fi
  done
  echo "24 blob files for 8 circuits in $cache"

  # A runtime fork other than the published one (a rehearsal, or the next
  # fork before its key is published) has its own blockchain key.
  if [ "${MESA_CIRCUITS_LIVE_KEY:-}" = none ]; then
    echo "MESA_CIRCUITS_LIVE_KEY=none: not comparing the blockchain wrap key with the live key"
    return 0
  fi
  local live="$root/data/vk/$MINA_PROFILE-mesa-blockchain-vk.json"
  [ -f "$live" ] || fail "no live key $live"
  local wrap_vks
  wrap_vks=$(count "$cache" 'vk-wrap-wrap-verification-key-blockchain-snark-*')
  [ "$wrap_vks" -eq 1 ] || fail "expected one blockchain wrap key, found $wrap_vks"
  python3 -I "$here/vk_match.py" "$live" \
    "$cache"/vk-wrap-wrap-verification-key-blockchain-snark-* |
    tail -1 ||
    fail "the blockchain wrap key is not the live $MINA_PROFILE key"
}

# The blob files, and nothing else, under the names the prover asks for.
link_blobs() {
  local cache=$1 dest=$2 f
  mkdir -p "$dest"
  for f in "$cache"/*_gates.json "$cache"/*_internal_vars.bin "$cache"/*_rows_rev.bin; do
    ln -sfn "$f" "$dest/$(basename "$f")"
  done
}

check_links() {
  local cache=$1 dest=$2 f n=0
  for f in "$cache"/*_gates.json "$cache"/*_internal_vars.bin "$cache"/*_rows_rev.bin; do
    cmp -s "$f" "$dest/$(basename "$f")" || fail "$dest/$(basename "$f") is missing or differs"
    n=$((n + 1))
  done
  echo "$n blob files in $dest"
}

if [ "${1:-}" = "--check" ]; then
  [ $# -eq 2 ] || fail "usage: generate.sh --check <out-dir>"
  out=$(realpath "$2")
  check "$out/tmp/coda_cache_dir"
  if [ -d "$out/circuit-blobs/$circuits_dir" ]; then
    check_links "$out/tmp/coda_cache_dir" "$out/circuit-blobs/$circuits_dir"
  fi
  exit 0
fi

[ $# -ge 1 ] || fail "usage: generate.sh <out-dir> [fork-config.json]"
out=$(realpath -m "$1")
cfg=$(realpath "${2:-$here/mesa-mainnet-fork.json}")
cache="$out/tmp/coda_cache_dir"
[ ! -e "$cache" ] || fail "$cache exists; use an empty output directory"

mkdir -p "$out/tmp" "$out/conf"
export TMPDIR="$out/tmp"
export MINA_DUMP_CIRCUIT_DATA=1
export MINA_USE_MMAP_CACHE=0

if ! "$root/scripts/ocaml-mina" transaction-snark-profiler --repeat 1 \
  >"$out/transaction.log" 2>&1; then
  n=$(count "$cache" 'step-step-proving-key-transaction-snark-*_gates.json')
  w=$(count "$cache" 'wrap-wrap-proving-key-transaction-snark-*_gates.json')
  if [ "$n" -ne 5 ] || [ "$w" -ne 1 ]; then
    tail -20 "$out/transaction.log" >&2
    fail "transaction-snark-profiler failed after dumping $n step and $w wrap circuits (expected 5 and 1)"
  fi
  echo "transaction-snark-profiler failed while proving, as expected, after dumping all 6 transaction circuits"
fi

"$root/scripts/ocaml-mina" internal test-genesis-block-generation \
  --config-file "$cfg" \
  --config-directory "$out/conf" \
  --genesis-ledger-dir "$out/conf/genesis" \
  >"$out/blockchain.log" 2>&1 ||
  {
    tail -20 "$out/blockchain.log" >&2
    fail "test-genesis-block-generation failed"
  }

check "$cache"
ls -1 "$cache"/*_gates.json
link_blobs "$cache" "$out/circuit-blobs/$circuits_dir"
check_links "$cache" "$out/circuit-blobs/$circuits_dir"
echo "MINA_CIRCUIT_BLOBS_BASE_DIR=$out/circuit-blobs"
