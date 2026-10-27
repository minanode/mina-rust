#!/usr/bin/env bash
# The relay host's firewall, with ufw. Read it, then run it once as root:
#
#   sudo SSH_FROM=<your address or range> ./firewall.sh
#
# Open: SSH; 80 and 443 for Caddy (ACME, HTTPS signalling, HTTP/3); the
# node's libp2p port, so devnet peers can dial in; its WebRTC UDP ports
# (relay.env WEBRTC_UDP_PORTS), which browsers connect to; TURN and STUN on
# 3478, UDP and TCP. Closed: everything else, the node's HTTP and signalling
# ports among them (on 127.0.0.1 already; the HTTP port has the operator API)
# and TURN's relay ports
# (traffic between TURN and the node on this host goes over the loopback
# interface, which ufw allows).
#
# Check the ports against /etc/mina-relay/relay.env and turnserver.conf
# first. Keep a console open (the provider's web console) in case SSH is cut.
set -euo pipefail

SSH_FROM=${SSH_FROM:-any}
MINA_LIBP2P_PORT=${MINA_LIBP2P_PORT:-8302}
WEBRTC_UDP_PORTS=${WEBRTC_UDP_PORTS:-40000-40999}
TURN_PORT=${TURN_PORT:-3478}

ufw default deny incoming
ufw default allow outgoing
if [[ $SSH_FROM == any ]]; then
	ufw allow 22/tcp comment 'ssh'
else
	ufw allow from "$SSH_FROM" to any port 22 proto tcp comment 'ssh'
fi
ufw allow 80/tcp comment 'caddy: ACME and redirect'
ufw allow 443/tcp comment 'caddy: HTTPS signalling'
ufw allow 443/udp comment 'caddy: HTTP/3'
ufw allow "$MINA_LIBP2P_PORT"/tcp comment 'mina libp2p'
ufw allow "${WEBRTC_UDP_PORTS/-/:}"/udp comment 'mina WebRTC'
ufw allow "$TURN_PORT"/udp comment 'coturn'
ufw allow "$TURN_PORT"/tcp comment 'coturn'
ufw --force enable
ufw status verbose
