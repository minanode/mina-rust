#!/usr/bin/env bash
# Run our node on mainnet in a node slot, feeding the e2e archive process.
# node.sh HTTP_PORT LIBP2P_PORT [extra node args]; MINA is the binary. The
# home ($E2E/home, with the outbox in archive-outbox/) is kept across runs.
set -euo pipefail
source "$(dirname "$0")/env.sh"
http=$1 p2p=$2
shift 2
export MINA_ARCHIVE_ADDRESS=http://127.0.0.1:$ARCHIVE_PORT
export TMPDIR=$E2E/tmp
mkdir -p "$TMPDIR"
exec "$(dirname "$0")/../lock.sh" node --log "$E2E/node.log" -- \
	"${MINA:?set MINA to the mina binary}" --network mainnet node -d "$E2E/home" \
	-p "$http" --libp2p-port "$p2p" \
	--genesis-config "${MINA_GENESIS_CONFIG:?set MINA_GENESIS_CONFIG (mesa-mainnet-without-ledgers.json)}" \
	--peer-list-url https://storage.googleapis.com/seed-lists/mainnet_seeds.txt \
	--disable-filesystem-logging --archive-archiver-process "$@"
