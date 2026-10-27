#!/usr/bin/env bash
# Start the relay node. mina-relay.service runs it; it is not for a shell.
#
# It reads the libp2p key from the credential systemd hands the service
# ($CREDENTIALS_DIRECTORY/p2p_key, from LoadCredential=) into
# MINA_P2P_SEC_KEY, so that the key is in no command line, and execs the
# node next to it (./mina) with the settings of /etc/mina-relay/relay.env.
# The node's work directory, frontier store and verifier index cache go to
# $STATE_DIRECTORY (/var/lib/mina-relay).
#
# Settings (relay.env):
#   NETWORK           devnet (default) or mainnet
#   HTTP_PORT         the node's HTTP port, on 127.0.0.1 (the node's default
#                     --http-addr): its whole API, for the operator, and the
#                     health checks Caddy proxies (default 3000)
#   WEBRTC_SIGNAL_PORT  --webrtc-signal-port: a listener on 127.0.0.1 that
#                     serves only the browsers' WebRTC signalling (RL1),
#                     which Caddy proxies; empty for none, when Caddy must
#                     proxy the signalling to HTTP_PORT instead
#   MINA_LIBP2P_PORT  the node's libp2p port (default 8302)
#   MAX_PEERS         peers, browsers included (default 100)
#   ICE_SERVERS_FILE  --webrtc-ice-servers: a JSON list; [] for none
#   WEBRTC_UDP_PORTS  --webrtc-udp-ports MIN-MAX, opened in the firewall
#   WEBRTC_NAT_IP     --webrtc-nat-ip: the public address, when the
#                     interface has a private one behind a 1:1 NAT
#   EXTERNAL_IP       --libp2p-external-ip: the public address, for gossip
#   EXTRA_ARGS        more node flags, split on spaces
set -euo pipefail

die() {
	echo "start-relay.sh: $*" >&2
	exit 1
}
[[ -n ${CREDENTIALS_DIRECTORY:-} ]] || die "no CREDENTIALS_DIRECTORY: run by mina-relay.service"
[[ -n ${STATE_DIRECTORY:-} ]] || die "no STATE_DIRECTORY: run by mina-relay.service"
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
mina=${MINA:-$here/mina}
[[ -x $mina ]] || die "$mina is not executable"

key=$CREDENTIALS_DIRECTORY/p2p_key
[[ -s $key ]] || die "no libp2p key in the p2p_key credential"
MINA_P2P_SEC_KEY=$(<"$key")
export MINA_P2P_SEC_KEY
export HOME=$STATE_DIRECTORY

args=(
	--network "${NETWORK:-devnet}" node
	-d "$STATE_DIRECTORY/node"
	-p "${HTTP_PORT:-3000}"
	--libp2p-port "${MINA_LIBP2P_PORT:-8302}"
	--max-peers "${MAX_PEERS:-100}"
	--frontier-store
	--disable-filesystem-logging
)
[[ -z ${WEBRTC_SIGNAL_PORT:-} ]] || args+=(--webrtc-signal-port "$WEBRTC_SIGNAL_PORT")
[[ -z ${ICE_SERVERS_FILE:-} ]] || args+=(--webrtc-ice-servers "$ICE_SERVERS_FILE")
[[ -z ${WEBRTC_UDP_PORTS:-} ]] || args+=(--webrtc-udp-ports "$WEBRTC_UDP_PORTS")
[[ -z ${WEBRTC_NAT_IP:-} ]] || args+=(--webrtc-nat-ip "$WEBRTC_NAT_IP")
[[ -z ${EXTERNAL_IP:-} ]] || args+=(--libp2p-external-ip "$EXTERNAL_IP")
# shellcheck disable=SC2206 # EXTRA_ARGS is a list of flags.
args+=(${EXTRA_ARGS:-})
exec "$mina" "${args[@]}"
