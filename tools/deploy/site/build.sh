#!/usr/bin/env bash
# Build the public devnet web node (minaweb.org): ../../regression/webnode/
# build.sh with the public relays over HTTPS, the asset host for the WASM
# and circuit files, the ICE servers and the static host's headers. See
# README.md here.
#
#   RELAYS="relay1.minaweb.org=12D3KooW... relay2.minaweb.org=12D3KooW..." \
#       tools/deploy/site/build.sh [OUT]
#
# It compiles the WASM, so on the shared machine run it under lock.sh build,
# with its own CARGO_TARGET_DIR. OUT (default
# .work/webnode/builds/<commit>-devnet-public) gets browser/, the site, and
# upload/, the files for the asset host.
#
# Environment (defaults: the minaweb.org names):
#   RELAYS         required: each relay as NAME=PEER_ID or NAME:PORT=PEER_ID,
#                  separated by spaces: its DNS name, HTTPS port and libp2p
#                  peer id (../relay/README.md, "The peer id"). The page
#                  dials every one, so it keeps syncing while one is down
#   RELAY_PEER_ID, RELAY_DOMAIN  one relay instead of RELAYS, as before:
#                  its peer id and name (such as relay1.minaweb.org), both
#   RELAY_PORT     the HTTPS port of a relay without one (default 443)
#   ASSETS_URL     the asset host, R2 behind a custom domain (default
#                  https://assets.minaweb.org): the WASM at ASSETS_URL/wasm,
#                  circuit files at ASSETS_URL/circuit-blobs
#   HOSTING        pages (default): Cloudflare Pages, which takes no file over
#                  25 MiB, so the WASM goes to the asset host; _headers.
#                  netlify: the WASM in the bundle; _headers and _redirects.
#                  caddy: everything in the bundle, circuits too; site.caddy
#                  has the headers
#   CIRCUITS_URL   overrides ASSETS_URL/circuit-blobs (empty: the bundle's)
#   ICE_SERVERS    the page's STUN and TURN servers (a JSON file; default
#                  none, for the node's built-in STUN servers). List each
#                  relay's TURN server. It holds the TURN credentials: keep
#                  it out of git. The shape is in ice-servers.example.json
#   ICE_SERVERS_URLS  build.sh's: lists the page fetches instead, such as
#                  each relay's credential service (default none)
#   PROVER_CIRCUITS  build.sh's: prover circuit files to ship and pin
#   NATIVE         a native node's HTTP API for the explorer pages (default
#                  none)
#   WASM_FEATURES  build.sh's: the WASM's cargo features (default none)
#   and build.sh's CONTROLS (default view-only), FRONTIER_STORE (default on),
#   ALLOW_DIRTY and CARGO_BUILD_JOBS.
set -euo pipefail

die() {
	echo "site/build.sh: $*" >&2
	exit 1
}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(git -C "$HERE" rev-parse --show-toplevel)

RELAYS=${RELAYS:-}
RELAY_PORT=${RELAY_PORT:-443}
ASSETS_URL=${ASSETS_URL:-https://assets.minaweb.org}
ASSETS_URL=${ASSETS_URL%/}
HOSTING=${HOSTING:-pages}

# One relay, as before: RELAY_PEER_ID and RELAY_DOMAIN.
if [[ -n ${RELAY_PEER_ID:-}${RELAY_DOMAIN:-} ]]; then
	[[ -z $RELAYS ]] || die "set RELAYS or RELAY_PEER_ID and RELAY_DOMAIN, not both"
	[[ -n ${RELAY_DOMAIN:-} ]] || die "set RELAY_DOMAIN with RELAY_PEER_ID, such as relay1.minaweb.org"
	RELAYS=$RELAY_DOMAIN=${RELAY_PEER_ID:-}
fi
[[ -n $RELAYS ]] || die "set RELAYS to each relay's NAME=PEER_ID, separated by spaces"
[[ $RELAY_PORT =~ ^[0-9]+$ ]] || die "RELAY_PORT is not a number: $RELAY_PORT"
RELAY=
for entry in $RELAYS; do
	[[ $entry =~ ^([A-Za-z0-9.-]+)(:([0-9]+))?=(12D3KooW[1-9A-HJ-NP-Za-km-z]{44})$ ]] ||
		die "a RELAYS entry is NAME=PEER_ID or NAME:PORT=PEER_ID, the peer id 12D3KooW... (52 characters): $entry"
	RELAY+="${RELAY:+,}/dns4/${BASH_REMATCH[1]}/tcp/${BASH_REMATCH[3]:-$RELAY_PORT}/webrtc/https/p2p/${BASH_REMATCH[4]}"
done
# A page served over HTTPS can fetch only HTTPS URLs.
[[ $ASSETS_URL == https://* ]] || die "ASSETS_URL must be https: $ASSETS_URL"
case $HOSTING in
pages) WASM_URL=$ASSETS_URL/wasm ;;
netlify) WASM_URL= ;;
caddy) WASM_URL= ;;
*) die "HOSTING is pages, netlify or caddy, not $HOSTING" ;;
esac
if [[ -z ${CIRCUITS_URL+set} ]]; then
	CIRCUITS_URL=$([[ $HOSTING == caddy ]] || echo "$ASSETS_URL/circuit-blobs")
fi
[[ -z $CIRCUITS_URL || $CIRCUITS_URL == https://* ]] || die "CIRCUITS_URL must be https: $CIRCUITS_URL"

COMMIT=$(git -C "$REPO" rev-parse HEAD)
OUT=${1:-$REPO/.work/webnode/builds/${COMMIT:0:12}-devnet-public}
export NETWORK=devnet
export RELAY
export WASM_URL CIRCUITS_URL
export ICE_SERVERS=${ICE_SERVERS:-}
export ICE_SERVERS_URLS=${ICE_SERVERS_URLS:-}
export NATIVE=${NATIVE:-}
export CONTROLS=${CONTROLS:-}
# Extra cargo features for the WASM; its defaults include ready events.
export WASM_FEATURES=${WASM_FEATURES-}
"$REPO/tools/regression/webnode/build.sh" "$OUT"

B=$OUT/browser
# The host's headers, with the page's Content-Security-Policy that build.sh
# made from this build's origins (build-info.json). Caddy takes it from
# SITE_CSP (site.caddy).
CSP_PY=$REPO/tools/regression/webnode/csp.py
case $HOSTING in
pages) python3 -B "$CSP_PY" headers "$HERE/_headers" "$B/build-info.json" >"$B/_headers" ;;
netlify)
	python3 -B "$CSP_PY" headers "$HERE/_headers" "$B/build-info.json" >"$B/_headers"
	cp "$HERE/_redirects" "$B/"
	;;
caddy) ;;
esac
# For a local `serve` only.
rm -f "$B/serve.json"

echo
echo "site: $B ($HOSTING)"
echo "relays: ${RELAY//,/ }"
echo "Content-Security-Policy: $(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["contentSecurityPolicy"])' "$B/build-info.json")"
[[ $HOSTING != caddy ]] || echo "for site.caddy: SITE_CSP, this Content-Security-Policy (build-info.json's contentSecurityPolicy)"
if [[ -d $OUT/upload ]]; then
	echo "for the asset host ($ASSETS_URL), keeping the paths:"
	(cd "$OUT/upload" && find . -type f | sort | sed 's|^\./|  |')
	echo "see README.md here, \"The asset host\", for the upload commands"
fi
