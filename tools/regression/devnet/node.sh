#!/usr/bin/env bash
# Run a native node that follows Mesa devnet: a soak node, or a relay for web
# nodes, which is the same node with browsers pointed at its HTTP port (it
# serves WebRTC signaling there). See ../README.md, "Devnet: a relay and a web
# node".
#
#   tools/regression/devnet/node.sh RUN_DIR HTTP_PORT LIBP2P_PORT [KEY_FILE]
#
# RUN_DIR      the node's data directory is RUN_DIR/home; RUN_DIR/start gets
#              the start time and RUN_DIR/peer-id the peer id. HOME and TMPDIR
#              are RUN_DIR/home-env and RUN_DIR/tmp, which keeps the verifier
#              index cache the node writes (home-env/.mina/circuit-blobs) out
#              of the shared ~/.mina; webnode-build.sh can copy it from there.
# KEY_FILE     a p2p secret key file, so that the peer id, and with it the
#              relay's WebRTC address in a web build, stays the same across
#              restarts. It is made (mode 600) if missing. The key goes to the
#              node in MINA_P2P_SEC_KEY, never on the command line, and is
#              never printed. Without it each start gets a new peer id.
#
# The node runs in the foreground, its log on stdout: redirect it, or run it
# under ../lock.sh, or under watch.sh for a soak. It uses the devnet seeds
# (`--network devnet`) and the Mesa devnet genesis config without ledgers in
# genesis_ledgers/, the one the binary embeds. Block proofs are verified.
#
# Environment:
#   MINA         the mina binary (default: target/release/mina of this checkout)
#   GENESIS      the genesis config (default: this checkout's
#                genesis_ledgers/mesa-devnet-without-ledgers.json); set it, and
#                MINA, when running a copy of this script outside a checkout
#   EXTRA_ARGS   more node flags, split on spaces (for example `--max-peers 50`)
set -euo pipefail

usage() { sed -n '2,/^[^#]/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; }
if [[ ${1:-} == -h || ${1:-} == --help || $# -lt 3 || $# -gt 4 ]]; then
	usage
	[[ ${1:-} == -h || ${1:-} == --help ]] && exit 0
	exit 2
fi
run=$1 http=$2 p2p=$3 key=${4:-}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
mina=${MINA:-$repo/target/release/mina}
genesis=${GENESIS:-$repo/genesis_ledgers/mesa-devnet-without-ledgers.json}
[[ -x $mina ]] || {
	echo "node.sh: $mina is not executable: set MINA" >&2
	exit 2
}
[[ -f $genesis ]] || {
	echo "node.sh: no genesis config $genesis: set GENESIS" >&2
	exit 2
}

mkdir -p "$run/home" "$run/home-env" "$run/tmp"
run=$(cd "$run" && pwd)
export HOME=$run/home-env TMPDIR=$run/tmp
if [[ -n $key ]]; then
	if [[ ! -s $key ]]; then
		(
			umask 077
			"$mina" keys p2p-key-pair | sed -n 's/^secret key: *//p' >"$key.new"
		)
		[[ -s $key.new ]] || {
			echo "node.sh: could not make a key in $key" >&2
			exit 1
		}
		mv "$key.new" "$key"
		echo "node.sh: made a new p2p key in $key" >&2
	fi
	MINA_P2P_SEC_KEY=$(cat "$key")
	export MINA_P2P_SEC_KEY
	"$mina" keys p2p-key-pair | sed -n 's/^libp2p_id: *//p' >"$run/peer-id"
	echo "node.sh: peer id $(cat "$run/peer-id"), WebRTC address" \
		"/ip4/127.0.0.1/tcp/$http/webrtc/http/p2p/$(cat "$run/peer-id")" >&2
fi
date -u +%FT%T.%NZ >"$run/start"
# shellcheck disable=SC2086 # EXTRA_ARGS is a list of flags.
exec "$mina" --network devnet node -d "$run/home" \
	-p "$http" --libp2p-port "$p2p" \
	--genesis-config "$genesis" \
	--disable-filesystem-logging ${EXTRA_ARGS:-}
