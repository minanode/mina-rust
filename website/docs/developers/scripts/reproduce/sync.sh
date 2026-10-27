#!/usr/bin/env bash
set -euo pipefail
: "${NETWORK:?set NETWORK=mainnet or NETWORK=devnet}"
case $NETWORK in mainnet|devnet) ;; *) exit 2 ;; esac
HTTP=${HTTP:-3410}
P2P=${P2P:-8610}
RUN=${RUN:-$PWD/operator-$NETWORK}
mkdir -p "$RUN/data"
# Foreground: stop with Ctrl-C. Use a separate directory for each network.
tools/regression/lock.sh node -- target/release/mina node --network "$NETWORK" \
    --work-dir "$RUN/data" --port "$HTTP" --libp2p-port "$P2P" \
    --genesis-config "$PWD/genesis_ledgers/mesa-$NETWORK-without-ledgers.json" \
    --peer-list-url "https://storage.googleapis.com/seed-lists/${NETWORK}_seeds.txt" \
    --disable-filesystem-logging 2>&1 | tee "$RUN/node.log"
