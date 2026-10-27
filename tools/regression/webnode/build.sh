#!/usr/bin/env bash
# Build a web node, frontend and WASM, from one clean commit, and assemble it
# ready to serve. The page footer and build-info.json name that commit, and
# the script checks that the WASM and the bundle both carry it, so a served
# page can always be traced to its source. See ../README.md, "Building and
# serving a web node".
#
#   tools/regression/webnode/build.sh [OUT]
#
# It compiles, so on the shared machine run it under lock.sh build:
#
#   tools/regression/lock.sh build -- tools/regression/webnode/build.sh
#
# OUT (default $WORK/webnode/builds/<commit>-<network>) is replaced. Serve
# OUT/browser, from that directory, with the headers the WASM threads need:
#
#   npx -y serve@14 -s . -l <port> -c serve.json
#
# Any port or host works: the seed list is a path on the page's own origin.
# Open /loading-web-node and press "Start the web node".
#
# Environment:
#   NETWORK      mainnet (default) or devnet
#   RELAY        required: your relay's reachable WebRTC multiaddr
#   GENESIS      genesis config without ledgers (default
#                $REPO/genesis_ledgers/mesa-<network>-without-ledgers.json)
#   CONTROLS     MINA_FRONTEND_CONTROLS: empty for a view-only page (default),
#                or a comma-separated list of producer and send, or all
#   NATIVE       MINA_NATIVE_NODE_URL: the HTTP API of a native node whose
#                block explorer the page offers next to the web node (default
#                the soak node on this machine, http://127.0.0.1:3200; empty
#                for none). The visitor's browser calls it, so it must be
#                reachable from there and answer CORS requests
#   FRONTIER_STORE  on or off: whether the web node keeps its verified ledgers
#                and blocks in the browser's Origin Private File System and
#                restores from them on a reload (I5e). Default on for devnet,
#                off for mainnet (not measured there yet). A visitor overrides
#                it with localStorage webnodeFrontierStore=on|off
#   ICE_SERVERS  a JSON list of RTCIceServer (STUN and TURN servers) to ship
#                as /webnode-<network>/ice-servers.json, which the page hands
#                the web node at start (W1). Without it the node uses its
#                built-in STUN servers. The file is public: see
#                tools/deploy/relay/README.md on TURN credentials
#   CIRCUITS_URL where the WASM fetches circuit files (the verifier index
#                caches here; a producer's or SNARK worker's prover circuits),
#                compiled in as CIRCUIT_BLOBS_HTTP_PREFIX, for a host such as
#                https://circuits.example.org/circuit-blobs. Default:
#                /assets/webnode/circuit-blobs on the page's origin. With it
#                the caches go to OUT/upload/circuit-blobs, for that host,
#                which must send CORS headers (tools/deploy/site/README.md)
#                With CIRCUITS_URL the SHA-256 of every circuit file the build
#                ships is compiled into the WASM, which refuses a file
#                without its pin or with other bytes (W1 fix round)
#   PROVER_CIRCUITS  a directory of prover circuit files for this network
#                (*_gates.json, *_internal_vars.bin, *_rows_rev.bin), shipped
#                and pinned with the verifier caches, for a producer's or
#                SNARK worker's page. Without it, a build with CIRCUITS_URL
#                has no pinned prover circuits and refuses to load any
#   WASM_URL     a directory URL on another host the page loads the WASM
#                from, for a static host that caps files below its 35 MB
#                (Cloudflare Pages: 25 MiB). The WASM files then go to
#                OUT/upload/wasm, and the page gets the WASM's URL and SHA-256:
#                it checks the bytes before it runs them and refuses others.
#                Default: the bundle's assets/webnode/pkg
#   ALLOW_DIRTY  1 to build uncommitted changes; the stamp says -dirty
#   WASM_FEATURES  cargo features for the WASM (mina-node-web), such as
#                p2p-webrtc-ready-events; default none beyond its defaults
#   CARGO_BUILD_JOBS  default 8
#
# Needs: cargo +nightly-2026-07-15 with the wasm32-unknown-unknown target and
# rust-src, wasm-bindgen 0.2.106 (the version of the crate in Cargo.lock),
# node 24; npm ci installs frontend dependencies in this checkout when absent.
set -euo pipefail

TOOLCHAIN=nightly-2026-07-15
WASM_BINDGEN_VERSION=0.2.106

die() {
	echo "build.sh: $*" >&2
	exit 1
}
step() { echo "== $(date -u +%H:%M:%S) $*"; }

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(git -C "$HERE" rev-parse --show-toplevel)
WORK=$REPO/.work
NETWORK=${NETWORK:-mainnet}
RELAY=${RELAY:-}
GENESIS=${GENESIS:-$REPO/genesis_ledgers/mesa-$NETWORK-without-ledgers.json}
CONTROLS=${CONTROLS:-}
NATIVE=${NATIVE-http://127.0.0.1:3200}
FRONTIER_STORE=${FRONTIER_STORE:-$([[ $NETWORK == devnet ]] && echo on || echo off)}
ICE_SERVERS=${ICE_SERVERS:-}
CIRCUITS_URL=${CIRCUITS_URL:-}
CIRCUITS_URL=${CIRCUITS_URL%/}
WASM_URL=${WASM_URL:-}
WASM_URL=${WASM_URL%/}
PROVER_CIRCUITS=${PROVER_CIRCUITS:-}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}

case $NETWORK in
mainnet | devnet) ;;
*) die "NETWORK is mainnet or devnet, not $NETWORK" ;;
esac
[[ -n $RELAY ]] || die "set RELAY to your reachable relay's WebRTC multiaddr (see the reproduction guide)"
[[ $RELAY == /ip4/*/tcp/*/webrtc/http*/p2p/* || $RELAY == /dns4/*/tcp/*/webrtc/http*/p2p/* ]] || die "RELAY must be a WebRTC multiaddr"
[[ -f $GENESIS ]] || die "no genesis config at $GENESIS (set GENESIS)"
[[ $FRONTIER_STORE == on || $FRONTIER_STORE == off ]] || die "FRONTIER_STORE is on or off, not $FRONTIER_STORE"
url_ok() { [[ -z $1 || $1 =~ ^https?://[A-Za-z0-9.:/_~%-]+$ ]]; }
url_ok "$CIRCUITS_URL" || die "CIRCUITS_URL must be an http(s) URL without a query: $CIRCUITS_URL"
url_ok "$WASM_URL" || die "WASM_URL must be an http(s) URL without a query: $WASM_URL"

# The commit. Untracked files count: a new source file changes the build.
COMMIT=$(git -C "$REPO" rev-parse HEAD)
STAMP=$COMMIT
if [[ -n $(git -C "$REPO" status --porcelain) ]]; then
	[[ ${ALLOW_DIRTY:-} == 1 ]] || die "uncommitted changes in $REPO; commit them, or set ALLOW_DIRTY=1"
	STAMP=$COMMIT-dirty
fi
OUT=${1:-$WORK/webnode/builds/${COMMIT:0:12}-$NETWORK}

# Tools.
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}') || true
[[ $have == "$WASM_BINDGEN_VERSION" ]] ||
	die "wasm-bindgen $WASM_BINDGEN_VERSION needed, found '${have:-none}' (cargo install wasm-bindgen-cli --version $WASM_BINDGEN_VERSION)"
RUSTC_VERSION=$(rustc +$TOOLCHAIN --version) || die "toolchain $TOOLCHAIN missing (rustup toolchain install $TOOLCHAIN)"
NODE_VERSION=$(node --version) || die "node missing"
FRONTEND=$REPO/frontend
rustup component list --installed --toolchain "$TOOLCHAIN" | grep -q '^rust-src' ||
  die "rust-src missing: rustup component add rust-src --toolchain $TOOLCHAIN"
rustup target list --installed --toolchain "$TOOLCHAIN" | grep -qx wasm32-unknown-unknown ||
  die "WASM target missing: rustup target add wasm32-unknown-unknown --toolchain $TOOLCHAIN"
if [[ ! -e $FRONTEND/node_modules ]]; then
  (cd "$FRONTEND" && npm ci)
fi
# The ICE servers, checked as the page will check them: a list it would drop
# fails the build instead.
ICE_URL=
if [[ -n $ICE_SERVERS ]]; then
	[[ -f $ICE_SERVERS ]] || die "no ICE servers file $ICE_SERVERS"
	node --no-warnings --input-type=module -e '
		import { readFileSync } from "node:fs";
		const { parseIceServers } = await import(process.argv[1]);
		const servers = parseIceServers(JSON.parse(readFileSync(process.argv[2], "utf8")));
		if (typeof servers === "string") { console.error(servers); process.exit(1); }
	' "$FRONTEND/src/app/shared/helpers/ice-servers.helper.ts" "$ICE_SERVERS" ||
		die "ICE_SERVERS $ICE_SERVERS is not a list the page can use"
	ICE_URL=/webnode-$NETWORK/ice-servers.json
fi

step "building $STAMP for $NETWORK into $OUT"
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/webnode-build.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT

# 0. Circuit files, chosen before the WASM is built: with CIRCUITS_URL their
# SHA-256s are compiled into it, as CIRCUIT_BLOBS_SHA256 (W1 fix round). The
# verifier index cache (R11), if this machine has it: about 9 s less per
# start. A cache starts with the SHA-256 of the index source it was made from;
# only one made from the source this build embeds is shipped, as the node
# would fall back to building the index from any other. A native node of this
# commit writes them (~/.mina/circuit-blobs) when it first starts.
CIRCUITS=$([[ $NETWORK == mainnet ]] && echo 4.0.2mainnet || echo 4.0.2devnet)
SOURCES=$REPO/crates/ledger/src/proofs/data
CSTAGE=$STAGE/circuit-blobs/$CIRCUITS
mkdir -p "$CSTAGE"
for kind in block:blockchain transaction:transaction; do
	cache=$HOME/.mina/circuit-blobs/$CIRCUITS/${kind%%:*}_verifier_index.postcard
	source=$SOURCES/${NETWORK}_${kind##*:}_verifier_index.json
	[[ -f $cache ]] || {
		echo "no verifier cache $cache: the node builds the index at start"
		continue
	}
	made_from=$(head -c 32 "$cache" | od -An -tx1 -v | tr -d ' \n')
	if [[ $made_from != "$(sha256sum "$source" | awk '{print $1}')" ]]; then
		echo "not shipping $cache: made from another index source than $source"
		continue
	fi
	cp "$cache" "$CSTAGE/"
done
if [[ -n $PROVER_CIRCUITS ]]; then
	[[ -d $PROVER_CIRCUITS ]] || die "PROVER_CIRCUITS is not a directory: $PROVER_CIRCUITS"
	cp -L "$PROVER_CIRCUITS"/* "$CSTAGE/"
fi
PINS=
if [[ -n $CIRCUITS_URL ]]; then
	PINS=$(cd "$STAGE/circuit-blobs" && find . -type f | sed 's|^\./||' | sort |
		while read -r f; do printf '%s=%s;' "$f" "$(sha256sum "$f" | awk '{print $1}')"; done)
	PINS=${PINS:--}
	echo "pinning $(find "$CSTAGE" -type f | wc -l) circuit files in the WASM"
fi

# 1. WASM. Touching build.rs reruns vergen, which embeds the commit: cargo
# does not rerun it when HEAD moves in a worktree. CIRCUIT_BLOBS_HTTP_PREFIX
# and CIRCUIT_BLOBS_SHA256 are read at compile time; cargo rebuilds what
# reads them when they change.
step "WASM"
touch "$REPO/crates/node/build.rs"
(
	if [[ -n $CIRCUITS_URL ]]; then
		export CIRCUIT_BLOBS_HTTP_PREFIX=$CIRCUITS_URL CIRCUIT_BLOBS_SHA256=$PINS
	else
		unset CIRCUIT_BLOBS_HTTP_PREFIX CIRCUIT_BLOBS_SHA256
	fi
	cd "$REPO/crates/node/web"
	# .cargo/config.toml there sets build-std, atomics and the 4 GiB memory.
	cargo +$TOOLCHAIN build --release --target wasm32-unknown-unknown \
		${WASM_FEATURES:+--features "$WASM_FEATURES"} \
		--config 'target.wasm32-unknown-unknown.rustflags=["-C","link-arg=--export=__heap_base"]'
)
TARGET_DIR=$(cd "$REPO" && cargo metadata --format-version 1 --no-deps |
	python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
WASM_IN=$TARGET_DIR/wasm32-unknown-unknown/release/mina_node_web.wasm
wasm-bindgen --keep-debug --web --out-dir "$STAGE/pkg" "$WASM_IN"
grep -qa "$COMMIT" "$STAGE/pkg/mina_node_web_bg.wasm" ||
	die "the WASM does not embed $COMMIT: its build_env would name another commit"
if [[ -n $CIRCUITS_URL ]]; then
	grep -qa "$CIRCUITS_URL" "$STAGE/pkg/mina_node_web_bg.wasm" ||
		die "the WASM does not embed CIRCUITS_URL $CIRCUITS_URL"
	grep -qaF "$PINS" "$STAGE/pkg/mina_node_web_bg.wasm" ||
		die "the WASM does not embed the circuit pins"
fi

# The WASM under a name of its content, which the glue loads it by, so that a
# server can let browsers keep it for good (serve.json): another build is
# another name. Then the encodings a server can send as they are, without
# compressing 32 MB per request: brotli (node's zlib, quality 11, a 16 MiB
# window), zstd -19 (an 8 MiB window, the most browsers take) and gzip -9.
step "name and compress the WASM"
WASM_SHA=$(sha256sum "$STAGE/pkg/mina_node_web_bg.wasm" | awk '{print $1}')
WASM_NAME=mina_node_web_bg.${WASM_SHA:0:16}.wasm
GLUE=$STAGE/pkg/mina_node_web.js
[[ $(grep -c "'mina_node_web_bg\.wasm'" "$GLUE") == 1 ]] ||
	die "the glue does not name mina_node_web_bg.wasm exactly once"
mv "$STAGE/pkg/mina_node_web_bg.wasm" "$STAGE/pkg/$WASM_NAME"
# The glue's own default stays next to it, even with WASM_URL: there the page
# passes it the bytes it checked (wasm-integrity.helper.ts), and anything
# that would load the WASM unchecked asks the site, which hasn't got it.
sed -i "s|'mina_node_web_bg\.wasm'|'$WASM_NAME'|" "$GLUE"
PAGE_WASM_URL= PAGE_WASM_SHA256=
if [[ -n $WASM_URL ]]; then
	PAGE_WASM_URL=$WASM_URL/$WASM_NAME PAGE_WASM_SHA256=$WASM_SHA
fi
WASM=$STAGE/pkg/$WASM_NAME
node -e '
	const fs = require("fs"), zlib = require("zlib"), C = zlib.constants;
	const data = fs.readFileSync(process.argv[1]);
	fs.writeFileSync(process.argv[1] + ".br", zlib.brotliCompressSync(data, { params: {
		[C.BROTLI_PARAM_QUALITY]: 11, [C.BROTLI_PARAM_LGWIN]: 24,
		[C.BROTLI_PARAM_SIZE_HINT]: data.length } }));
' "$WASM" &
br=$!
zstd -q -19 -k "$WASM" -o "$WASM.zst" &
zst=$!
gzip -9 -k -n "$WASM" &
gz=$!
wait $br && wait $zst && wait $gz || die "compressing the WASM failed"

# 2. Frontend. Paths, not URLs, for the genesis and the seed list, so the
# build serves from any origin.
step "frontend"
(
	cd "$FRONTEND"
	npx ng build --configuration webnode-local --output-path "$STAGE/dist" \
		--define MINA_WEBNODE_NETWORK="\"$NETWORK\"" \
		--define MINA_WEBNODE_GENESIS_URL="\"/webnode-$NETWORK/genesis.json\"" \
		--define MINA_WEBNODE_BOOTNODES="\"$RELAY\"" \
		--define MINA_WEBNODE_SEED_URLS="\"/webnode-$NETWORK/peers.txt\"" \
		--define MINA_FRONTEND_COMMIT="\"$STAMP\"" \
		--define MINA_FRONTEND_CONTROLS="\"$CONTROLS\"" \
		--define MINA_NATIVE_NODE_URL="\"$NATIVE\"" \
		--define MINA_WEBNODE_FRONTIER_STORE="\"$FRONTIER_STORE\"" \
		--define MINA_WEBNODE_ICE_SERVERS_URL="\"$ICE_URL\"" \
		--define MINA_WEBNODE_WASM_URL="\"$PAGE_WASM_URL\"" \
		--define MINA_WEBNODE_WASM_SHA256="\"$PAGE_WASM_SHA256\""
)
if [[ -n $PAGE_WASM_SHA256 ]]; then
	grep -lq "$PAGE_WASM_SHA256" "$STAGE/dist/browser"/*.js ||
		die "the bundle does not pin the WASM's SHA-256"
fi
B=$STAGE/dist/browser
grep -lq "$STAMP" "$B"/*.js || die "the bundle does not carry the stamp $STAMP"

# 3. Assemble.
step "assemble"
rm -rf "$B/assets/webnode/pkg"
cp -r "$STAGE/pkg" "$B/assets/webnode/pkg"
mkdir -p "$B/webnode-$NETWORK"
cp "$GENESIS" "$B/webnode-$NETWORK/genesis.json"
echo "$RELAY" >"$B/webnode-$NETWORK/peers.txt"
[[ -z $ICE_SERVERS ]] || cp "$ICE_SERVERS" "$B$ICE_URL"
cp "$HERE/serve.json" "$B/"
# Files for other hosts, when the bundle doesn't carry them.
UPLOAD=$STAGE/dist/upload
if [[ -n $WASM_URL ]]; then
	mkdir -p "$UPLOAD/wasm"
	mv "$B/assets/webnode/pkg/$WASM_NAME"* "$UPLOAD/wasm/"
fi
CIRCUIT_DEST=$B/assets/webnode/circuit-blobs
[[ -z $CIRCUITS_URL ]] || CIRCUIT_DEST=$UPLOAD/circuit-blobs
if [[ -n $(ls -A "$CSTAGE") ]]; then
	mkdir -p "$CIRCUIT_DEST"
	cp -r "$CSTAGE" "$CIRCUIT_DEST/"
fi

sha() { sha256sum "$1" | awk '{print $1}'; }
size() { stat -c %s "$1"; }
W=$B/assets/webnode/pkg/$WASM_NAME
[[ -z $WASM_URL ]] || W=$UPLOAD/wasm/$WASM_NAME
python3 - "$B/build-info.json" <<EOF
import json, sys
json.dump({
    "commit": "$COMMIT",
    "stamp": "$STAMP",
    "network": "$NETWORK",
    "relay": "$RELAY",
    "seedList": "/webnode-$NETWORK/peers.txt",
    "genesis": {"path": "/webnode-$NETWORK/genesis.json", "sha256": "$(sha "$GENESIS")"},
    "controls": "$CONTROLS" or "view-only",
    "nativeNode": "$NATIVE" or None,
    "frontierStore": "$FRONTIER_STORE" == "on",
    "iceServers": "$ICE_URL" or None,
    "circuitsUrl": "$CIRCUITS_URL" or "/assets/webnode/circuit-blobs",
    "circuitPins": "$PINS".count("=") if "$CIRCUITS_URL" else None,
    "wasm": {"path": "${WASM_URL:-/assets/webnode/pkg}/$WASM_NAME", "sha256": "$(sha "$W")",
             "bytes": $(size "$W"), "br": $(size "$W.br"), "zst": $(size "$W.zst"),
             "gz": $(size "$W.gz"),
             "rustc": "$RUSTC_VERSION", "wasmBindgen": "$WASM_BINDGEN_VERSION",
             "features": "${WASM_FEATURES:-}" or None,
             "pinnedInPage": bool("$PAGE_WASM_SHA256")},
    "frontend": {"node": "$NODE_VERSION", "configuration": "webnode-local"},
    "builtAt": "$(date -u +%FT%TZ)",
}, open(sys.argv[1], "w"), indent=2)
EOF

rm -rf "$OUT"
mkdir -p "$(dirname "$OUT")"
mv "$STAGE/dist" "$OUT"
step "done: $OUT/browser"
cat "$OUT/browser/build-info.json"
echo
echo "serve: cd $OUT/browser && npx -y serve@14 -s . -l <port> -c serve.json"
