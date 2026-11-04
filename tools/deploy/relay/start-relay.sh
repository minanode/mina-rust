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
#   WEBRTC_SCTP_WINDOW  --webrtc-sctp-window: the most bytes in flight to
#                     one browser; empty for the default (1 MiB)
#   EXTERNAL_IP       --libp2p-external-ip: the public address, for gossip
#   SHARING_RELAY     on: --webrtc-sharing-relay, introduce web nodes that
#                     turned sharing on to each other, within its limits
#                     (MINAWEB.md, "Browser sharing"); off or empty (default):
#                     introduce no one
#   EXTRA_ARGS        more node flags, split on spaces; never
#                     --webrtc-sharing-relay (use SHARING_RELAY) or the
#                     0.20.1 release's --webrtc-signaling-relay
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
# 0.20.0 relays signalling between all its peers: it is not a public relay.
# The 0.20.1 release has --webrtc-signaling-relay. Later builds have only
# --webrtc-sharing-relay.
help=$("$mina" node --help)
[[ $help == *--webrtc-sharing-relay* || $help == *--webrtc-signaling-relay* ]] || die "use the 0.20.1 release or a build of main; 0.20.0 is not a public relay"
[[ ${EXTRA_ARGS:-} != *--webrtc-signaling-relay* ]] || die "remove --webrtc-signaling-relay from EXTRA_ARGS: it introduces every peer to every other. SHARING_RELAY=on introduces only pages that turned sharing on"
# Browser sharing has one switch, SHARING_RELAY, so that a review of
# relay.env sees it.
[[ ${EXTRA_ARGS:-} != *--webrtc-sharing-relay* ]] || die "set SHARING_RELAY=on instead of passing --webrtc-sharing-relay"
case ${SHARING_RELAY:-off} in
on) [[ $help == *--webrtc-sharing-relay* ]] || die "SHARING_RELAY=on needs a build with --webrtc-sharing-relay (MINAWEB-OPT-IN-SHARING)" ;;
off | "") ;;
*) die "SHARING_RELAY is on or off, not $SHARING_RELAY" ;;
esac

key=$CREDENTIALS_DIRECTORY/p2p_key
[[ -s $key ]] || die "no libp2p key in the p2p_key credential"
MINA_P2P_SEC_KEY=$(<"$key")
export MINA_P2P_SEC_KEY
export HOME=$STATE_DIRECTORY

args=(
	--network "${NETWORK:-devnet}" node
	-d "$STATE_DIRECTORY/node"
	-p "${HTTP_PORT:-3000}"
	--http-addr 127.0.0.1
	--webrtc-signal-addr 127.0.0.1
	--libp2p-port "${MINA_LIBP2P_PORT:-8302}"
	--max-peers "${MAX_PEERS:-100}"
	--frontier-store
	--disable-filesystem-logging
)
[[ -z ${WEBRTC_SIGNAL_PORT:-} ]] || args+=(--webrtc-signal-port "$WEBRTC_SIGNAL_PORT")
[[ -z ${ICE_SERVERS_FILE:-} ]] || args+=(--webrtc-ice-servers "$ICE_SERVERS_FILE")
[[ -z ${WEBRTC_UDP_PORTS:-} ]] || args+=(--webrtc-udp-ports "$WEBRTC_UDP_PORTS")
[[ -z ${WEBRTC_NAT_IP:-} ]] || args+=(--webrtc-nat-ip "$WEBRTC_NAT_IP")
[[ -z ${WEBRTC_SCTP_WINDOW:-} ]] || args+=(--webrtc-sctp-window "$WEBRTC_SCTP_WINDOW")
[[ -z ${EXTERNAL_IP:-} ]] || args+=(--libp2p-external-ip "$EXTERNAL_IP")
[[ ${SHARING_RELAY:-off} != on ]] || args+=(--webrtc-sharing-relay)
# shellcheck disable=SC2206 # EXTRA_ARGS is a list of flags.
args+=(${EXTRA_ARGS:-})
exec "$mina" "${args[@]}"
