#!/usr/bin/env bash
# Print the inputs OCaml Mina hashes into a network's chain id.
#
# `mina advanced constraint-system-digests` compiles the blockchain step
# circuit without the fork constants, so its blockchain-step digest is not the
# one a hard-forked network's chain id commits to. This runs
# `mina internal chain-id --from-config-hashes-only` under gdb, catches the
# Blake2b input (about 372 bytes), and splits it into the genesis state hash
# (with zero total currencies), the six constraint system digests, the genesis
# constants hash and the two version MD5s.
#
#   scripts/ocaml-chain-id-trace.sh devnet \
#     ~/.local/opt/mina-4.0.2-devnet-config/var/lib/coda/config_84b8d969.json
#   scripts/ocaml-chain-id-trace.sh mainnet \
#     ~/.local/opt/mina-4.0.2/var/lib/coda/config_84b8d969.json
#
# Needs gdb and the user-local OCaml 4.0.2 install used by scripts/ocaml-mina.
# x86_64 only: the breakpoint reads the buffer and its length from the System V
# argument registers of digestif_blake2b_update(ctx, buf, len), $rsi and $rdx.
set -euo pipefail

if [ "$(uname -m)" != x86_64 ]; then
  echo "$0: x86_64 only (reads the System V argument registers)" >&2
  exit 2
fi

if [ $# -ne 2 ]; then
  echo "usage: $0 <profile: mainnet|devnet> <config.json>" >&2
  exit 2
fi
profile=$1
config=$2

prefix="${OCAML_MINA_PREFIX:-$HOME/.local/opt/mina-4.0.2}"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Print each Blake2b input of 200-1000 bytes whole: no element limit and no
# "<repeats N times>" compression of runs of equal characters.
cat >"$work/gdb.cmd" <<'EOF'
set pagination off
set breakpoint pending on
set print elements unlimited
set print repeats unlimited
break digestif_blake2b_update if $rdx > 200 && $rdx < 1000
commands
silent
printf "LEN %d\n", $rdx
output *(char*)$rsi@$rdx
echo \n
continue
end
run
EOF

LD_LIBRARY_PATH="$prefix/usr/lib/x86_64-linux-gnu" \
  MINA_PROFILE="$profile" \
  MINA_LIBP2P_HELPER_PATH="$prefix/usr/local/bin/coda-libp2p_helper" \
  gdb -batch -x "$work/gdb.cmd" --args "$prefix/usr/local/bin/mina" \
  internal chain-id \
  --config-directory "$work/conf" \
  --config-file "$config" \
  --from-config-hashes-only >"$work/gdb.out" 2>"$work/gdb.err"

python3 -I "$here/ocaml-chain-id-split.py" "$work/gdb.out"
