#!/usr/bin/env bash
# Build the public devnet web node (minaweb.org): ../../regression/webnode/
# build.sh with the public relay over HTTPS, the asset host for the WASM and
# circuit files, the ICE servers and the static host's headers. See
# README.md here.
#
#   RELAY_PEER_ID=12D3KooW... tools/deploy/site/build.sh [OUT]
#
# It compiles the WASM, so on the shared machine run it under lock.sh build,
# with its own CARGO_TARGET_DIR. OUT (default
# .work/webnode/builds/<commit>-devnet-public) gets browser/, the site, and
# upload/, the files for the asset host.
#
# Environment (defaults: the minaweb.org names):
#   RELAY_PEER_ID  required: the relay's libp2p peer id
#                  (../relay/README.md, "The peer id")
#   RELAY_DOMAIN   the relay's name (default relay.minaweb.org)
#   RELAY_PORT     its HTTPS port (default 443)
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
#                  none, for the node's built-in STUN servers). It holds the
#                  TURN credential: keep it out of git. The shape is in
#                  ice-servers.example.json
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

RELAY_PEER_ID=${RELAY_PEER_ID:-}
RELAY_DOMAIN=${RELAY_DOMAIN:-relay.minaweb.org}
RELAY_PORT=${RELAY_PORT:-443}
ASSETS_URL=${ASSETS_URL:-https://assets.minaweb.org}
ASSETS_URL=${ASSETS_URL%/}
HOSTING=${HOSTING:-pages}

[[ $RELAY_PEER_ID =~ ^12D3KooW[1-9A-HJ-NP-Za-km-z]{44}$ ]] ||
	die "set RELAY_PEER_ID to the relay's peer id (12D3KooW..., 52 characters)"
[[ $RELAY_DOMAIN =~ ^[A-Za-z0-9.-]+$ ]] || die "RELAY_DOMAIN is not a host name: $RELAY_DOMAIN"
[[ $RELAY_PORT =~ ^[0-9]+$ ]] || die "RELAY_PORT is not a number: $RELAY_PORT"
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
export RELAY=/dns4/$RELAY_DOMAIN/tcp/$RELAY_PORT/webrtc/https/p2p/$RELAY_PEER_ID
export WASM_URL CIRCUITS_URL
export ICE_SERVERS=${ICE_SERVERS:-}
export NATIVE=${NATIVE:-}
export CONTROLS=${CONTROLS:-}
# Extra cargo features for the WASM; its defaults include ready events.
export WASM_FEATURES=${WASM_FEATURES-}
"$REPO/tools/regression/webnode/build.sh" "$OUT"

B=$OUT/browser
case $HOSTING in
pages) cp "$HERE/_headers" "$B/" ;;
netlify) cp "$HERE/_headers" "$HERE/_redirects" "$B/" ;;
caddy) ;;
esac
# For a local `serve` only.
rm -f "$B/serve.json"

echo
echo "site: $B ($HOSTING)"
echo "relay: $RELAY"
if [[ -d $OUT/upload ]]; then
	echo "for the asset host ($ASSETS_URL), keeping the paths:"
	(cd "$OUT/upload" && find . -type f | sort | sed 's|^\./|  |')
	echo "see README.md here, \"The asset host\", for the upload commands"
fi
