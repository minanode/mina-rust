#!/usr/bin/env bash
# P5.3 gate: start the browser worker in the run's network namespace, its
# only peer our relay at slot 26 (HTTP 3660). Run by tools/local-mesa-net/
# hold.py under the browser lock, once the run's nodes are up:
#
#   tools/regression/lock.sh browser -- python3 tools/local-mesa-net/hold.py \
#     --ticket LEASE/browser --log RUN/logs/browser-runner.log -- \
#     bash tools/webnode-snark-worker/gate-browser.sh RUN OUT
#
# PKG, BLOBS and optionally PORT (default 4236) and THREADS come from the
# environment. The relay's peer ID is read at start, from the run's keys.
# The browser runs at nice 5, as P5.2's worker did, so that the OCaml
# producer keeps the CPU first on a loaded host.
set -euo pipefail
run=$1
out=$2
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
peer=$(cat "$run/keys/rust-p2p.peerid")
exec nice -n 5 bash "$root/tools/local-mesa-net/enter.sh" "$run" env \
  RUN="$run" OUT="$out" PKG="${PKG:?}" BLOBS="${BLOBS:?}" \
  PORT="${PORT:-4236}" THREADS="${THREADS:-8}" \
  RELAY="/ip4/127.0.0.1/tcp/3660/webrtc/http/p2p/$peer" \
  node "$here/gate-browser.mjs"
