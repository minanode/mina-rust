#!/usr/bin/env bash
# Build a web node frontend for Mesa devnet that bootstraps through one relay.
# See ../README.md, "Devnet: a relay and a web node".
#
#   tools/regression/devnet/webnode-build.sh DIST RELAY PORT PKG
#
# DIST   the output directory, made anew; serve DIST/browser
# RELAY  the relay's WebRTC address,
#        /ip4/<ip>/tcp/<relay http port>/webrtc/http/p2p/<relay peer id>
# PORT   the suggested port for serving DIST/browser; seed URLs are relative
# PKG    the wasm-bindgen output for crates/node/web (its `pkg` directory);
#        the WASM picks the network at run time, so a mainnet build's will do
#
# The build has MINA_WEBNODE_NETWORK devnet, the Mesa devnet genesis config
# without ledgers (genesis_ledgers/, served as /webnode-devnet/genesis.json),
# RELAY as its bootnode and in /webnode-devnet/peers.txt, and serve.json for
# the COOP and COEP headers. It copies the native node's devnet verifier index
# cache from CIRCUIT_CACHE when there is one: a file that doesn't match is
# rejected by its digest, so this can only save time.
#
# Environment:
#   CIRCUIT_CACHE  the devnet verifier index cache to copy (default
#                  ~/.mina/circuit-blobs/berkeley-devnet; node.sh writes one
#                  to RUN_DIR/home-env/.mina/circuit-blobs/berkeley-devnet)
#
# Dependencies are installed in this checkout with npm ci when absent. On
# the shared machine, run it under ../lock.sh build.
set -euo pipefail

usage() { sed -n '2,/^[^#]/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; }
if [[ ${1:-} == -h || ${1:-} == --help ]]; then
	usage
	exit 0
fi
if [[ $# -ne 4 ]]; then
	usage >&2
	exit 2
fi
dist=$1 relay=$2 port=$3 pkg=$4
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
# The devnet `CircuitsConfig::directory_name` (crates/core/src/network.rs).
circuits_dir=berkeley-devnet
cache=${CIRCUIT_CACHE:-$HOME/.mina/circuit-blobs/$circuits_dir}

die() {
	echo "webnode-build.sh: $*" >&2
	exit 2
}
[[ $relay == /ip4/*/tcp/*/webrtc/http*/p2p/* || $relay == /dns4/*/tcp/*/webrtc/http*/p2p/* ]] ||
	die "RELAY is not a WebRTC address: $relay"
[[ $port =~ ^[0-9]+$ ]] || die "PORT is not a number: $port"
[[ -f $pkg/mina_node_web_bg.wasm && -f $pkg/mina_node_web.js ]] ||
	die "PKG has no mina_node_web_bg.wasm and mina_node_web.js: $pkg"
if [[ ! -d $repo/frontend/node_modules ]]; then
	(cd "$repo/frontend" && npm ci)
fi
mkdir -p "$(dirname "$dist")"
dist=$(cd "$(dirname "$dist")" && pwd)/$(basename "$dist")
pkg=$(cd "$pkg" && pwd)
rm -rf "$dist"

(
	cd "$repo/frontend"
	npx ng build --configuration webnode-local --output-path "$dist" \
		--define MINA_WEBNODE_NETWORK='"devnet"' \
		--define MINA_WEBNODE_GENESIS_URL='"/webnode-devnet/genesis.json"' \
		--define MINA_WEBNODE_BOOTNODES="\"$relay\"" \
		--define MINA_WEBNODE_SEED_URLS='"/webnode-devnet/peers.txt"'
)

browser=$dist/browser
rm -rf "$browser/assets/webnode/pkg"
cp -a "$pkg" "$browser/assets/webnode/pkg"
mkdir -p "$browser/webnode-devnet"
cp "$repo/genesis_ledgers/mesa-devnet-without-ledgers.json" "$browser/webnode-devnet/genesis.json"
printf '%s\n' "$relay" >"$browser/webnode-devnet/peers.txt"
cp "$repo/tools/regression/webnode/serve.json" "$browser/serve.json"
if compgen -G "$cache/*_verifier_index.postcard" >/dev/null; then
	mkdir -p "$browser/assets/webnode/circuit-blobs/$circuits_dir"
	cp "$cache"/*_verifier_index.postcard "$browser/assets/webnode/circuit-blobs/$circuits_dir/"
else
	echo "webnode-build.sh: no verifier index cache in $cache; the web node will build its own" >&2
fi

# What the build has compiled in, to check by eye.
echo "network, genesis, bootnodes and seed URLs compiled in:"
grep -hoE 'webNode(Network|GenesisUrl|BootNodes|SeedUrls):[^,]{0,160}' "$browser"/*.js | sort -u
(cd "$browser" && sha256sum assets/webnode/pkg/mina_node_web_bg.wasm webnode-devnet/genesis.json \
	assets/webnode/circuit-blobs/"$circuits_dir"/*.postcard 2>/dev/null) || true
echo "serve it: cd $browser && npx -y serve@14 -s . -l $port -c serve.json"
