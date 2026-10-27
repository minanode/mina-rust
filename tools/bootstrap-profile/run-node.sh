#!/usr/bin/env bash
# Start one native mainnet node from empty, with the genesis config without
# ledgers, and sample it until it has been synced for a minute.
#
# Usage: run-node.sh BINARY OUTDIR HTTP_PORT LIBP2P_PORT
#
# Writes OUTDIR/node.log, OUTDIR/pid, OUTDIR/start and the sampler's files.
# The node keeps running after the sampler stops: kill it by the PID in
# OUTDIR/pid.
#
# The genesis config is read from $ROOT/.work/genesis-mainnet, where ROOT
# defaults to the main checkout of the repository this script is in (git-ignored
# inputs live there, not in worktrees).
#
# The node joins mainnet. On the shared development machine, start this under
# a node slot, which the node inherits and holds until it exits:
#
#   tools/regression/lock.sh node -- tools/bootstrap-profile/run-node.sh ...
set -euo pipefail

if [ $# -ne 4 ]; then
    echo "usage: $0 BINARY OUTDIR HTTP_PORT LIBP2P_PORT" >&2
    exit 2
fi

BIN=$1
OUT=$2
HTTP=$3
P2P=$4
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -z "${ROOT:-}" ]; then
    # From a worktree too, git's common directory is the main checkout's .git.
    COMMON=$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir) || {
        echo "not in a git checkout: set ROOT to the main checkout" >&2
        exit 2
    }
    ROOT=$(dirname "$COMMON")
fi
GENESIS=$ROOT/.work/genesis-mainnet/mesa-mainnet-without-ledgers.json
if [ ! -f "$GENESIS" ]; then
    echo "missing $GENESIS: set ROOT to the checkout that has .work/" >&2
    exit 2
fi

mkdir -p "$OUT/home"
cd "$OUT"
date -u +%FT%T.%NZ >"$OUT/start"
setsid nohup "$BIN" --network mainnet node \
    -d "$OUT/home" \
    --genesis-config "$GENESIS" \
    --peer-list-url https://storage.googleapis.com/seed-lists/mainnet_seeds.txt \
    -p "$HTTP" --libp2p-port "$P2P" \
    --disable-filesystem-logging --verbosity debug \
    >"$OUT/node.log" 2>&1 </dev/null &
PID=$!
echo "$PID" >"$OUT/pid"
echo "node pid $PID"
setsid nohup python3 "$HERE/sample.py" --port "$HTTP" --pid "$PID" --out "$OUT" \
    >"$OUT/sampler.log" 2>&1 </dev/null &
echo "$!" >"$OUT/sampler.pid"
echo "sampler pid $!"
