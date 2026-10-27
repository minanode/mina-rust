#!/usr/bin/env bash
# Build the relay's `mina` from a clean checkout and check that it names the
# commit. RELAY_WEBRTC picks the WebRTC backend the relay serves browsers with:
#
#   rustrtc    the default: vendor/rustrtc, with ready events (WN2-RTC)
#   webrtc-rs  the fallback: the openmina webrtc-rs fork
#
#   tools/regression/lock.sh build -- env CARGO_BUILD_JOBS=8 nice -n 10 \
#       tools/deploy/relay/build-relay.sh
#
# Prints the binary's path. The native release builds the same default.
set -euo pipefail
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
repo=$(cd "$here/../../.." && pwd)
cd "$repo"
backend=${RELAY_WEBRTC:-rustrtc}
case $backend in
rustrtc) features=() ;;
webrtc-rs) features=(--features p2p-webrtc-rs) ;;
*)
	echo "build-relay.sh: RELAY_WEBRTC is webrtc-rs or rustrtc, not $backend" >&2
	exit 2
	;;
esac
echo "build-relay.sh: WebRTC backend $backend" >&2
cargo build --release -p mina-cli --bin mina "${features[@]}"
target=$(cargo metadata --format-version 1 --no-deps |
	python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
"$repo/tools/regression/check-build-env.sh" "$target/release/mina" "$(git rev-parse HEAD)" >&2
echo "$target/release/mina"
