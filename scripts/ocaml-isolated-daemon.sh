#!/usr/bin/env bash
# Run an OCaml Mina 4.0.2 mainnet daemon whose only peer is one given node,
# for example our Rust relay, to see whether OCaml can bootstrap from it.
#
#   WORK=/tmp/r6-ocaml PEER=/ip4/127.0.0.1/tcp/8503/p2p/12D3Koo... \
#     scripts/ocaml-isolated-daemon.sh [extra daemon flags...]
#
# WORK must hold daemon-config.json (the Mesa mainnet runtime config without
# peer_list_url), genesis/ (the genesis and epoch ledger tarballs, or links to
# them) and keys/p2p (from `mina libp2p generate-keypair`). The daemon writes
# its config directory to $WORK/$RUN and its stdout to $WORK/$RUN.out, where
# RUN defaults to "conf". Its PID goes to $WORK/$RUN.pid. Extra arguments are
# passed on to `mina daemon`.
#
# TMPDIR is set to a private directory, $WORK/tmp unless OCAML_TMPDIR names
# another, so the daemon's coda_cache_dir and its other temporary files stay
# out of the shared /tmp.
#
# Ports default to agent slot 4: libp2p 8640 (LIBP2P_PORT), client 8641
# (CLIENT_PORT), GraphQL 3440 (REST_PORT).
#
# On the shared development machine, when PEER is the shared test relay (libp2p
# 8503), start the daemon through a node slot, so that two agents don't load
# the relay with bootstrap RPCs and gossip at once:
#
#   WORK=... PEER=... tools/regression/lock.sh node -- \
#     scripts/ocaml-isolated-daemon.sh [extra daemon flags...]
#
# The daemon keeps lock.sh's PID, the one in $WORK/$RUN.pid, and holds the slot
# until it exits. --isolate-network keeps it off mainnet itself.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="${WORK:?set WORK to the run directory}"
peer="${PEER:?set PEER to the libp2p multiaddr of the only peer}"
run="${RUN:-conf}"

for input in daemon-config.json genesis keys/p2p; do
  if [[ ! -e "$work/$input" ]]; then
    echo "missing $work/$input" >&2
    exit 1
  fi
done
if grep -q peer_list_url "$work/daemon-config.json"; then
  echo "$work/daemon-config.json sets peer_list_url; remove it" >&2
  exit 1
fi

tmp="${OCAML_TMPDIR:-$work/tmp}"
mkdir -p "$tmp"
export TMPDIR="$tmp"
export MINA_LIBP2P_PASS="${MINA_LIBP2P_PASS:-r6pass}"
export MINA_PROFILE=mainnet

echo $$ >"$work/$run.pid"
exec "$here/ocaml-mina" daemon \
  --config-directory "$work/$run" \
  --config-file "$work/daemon-config.json" \
  --genesis-ledger-dir "$work/genesis" \
  --libp2p-keypair "$work/keys/p2p" \
  --peer "$peer" \
  --isolate-network true \
  --external-port "${LIBP2P_PORT:-8640}" \
  --client-port "${CLIENT_PORT:-8641}" \
  --rest-port "${REST_PORT:-3440}" \
  --external-ip 127.0.0.1 \
  --file-log-level Trace \
  --log-level Info \
  "$@" \
  >"$work/$run.out" 2>&1
