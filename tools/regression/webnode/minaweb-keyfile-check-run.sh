#!/usr/bin/env bash
# MINAWEB-KEYFILE-IMPORT: serve a devnet minaweb build (build.sh's OUT) on
# 127.0.0.1:PORT under its Content-Security-Policy, collecting violation
# reports, and run minaweb-keyfile-check.mjs against it with the bundled
# Chrome, node and puppeteer. The throwaway keys live in a 0700 directory
# under /dev/shm, deleted at the end.
#
#   PORT=4340 MINA_BIN=<native mina> BUNDLES=<bundles dir> \
#       tools/regression/webnode/minaweb-keyfile-check-run.sh BUILD OUT
#
# OCAML_FILE and OCAML_PASSWORD (optional) name a key file the OCaml node
# wrote, imported last; the repository's fixture is
# tests/files/accounts/keyfile-ocaml-4.0.2 (password in
# crates/node/account/tests/test_key_file.rs).
set -euo pipefail
umask 077
build=${1:?build directory}
out=${2:?output directory}
: "${PORT:?}" "${MINA_BIN:?}" "${BUNDLES:?}"
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
chrome=$BUNDLES/chrome-155.0.8059.84
mkdir -p "$out"
export OUT=$out
export BASE=http://127.0.0.1:$PORT
export CHROME=$chrome/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell
export LD_LIBRARY_PATH=$chrome/libs/usr/lib/x86_64-linux-gnu
export PUPPETEER_DIR=$BUNDLES/puppeteer-core-24
export MINA_BIN
KEY_DIR=$(mktemp -d /dev/shm/minaweb-keyfile-XXXXXX)
export KEY_DIR
export PROFILE_DIR=$KEY_DIR/profile
export CSP_REPORTS=$out/csp-reports.jsonl
: >"$CSP_REPORTS"
server_pid=
cleanup() {
	trap - EXIT
	[[ -z $server_pid ]] || kill "$server_pid" 2>/dev/null || true
	[[ -z $server_pid ]] || wait "$server_pid" 2>/dev/null || true
	rm -rf "$KEY_DIR"
}
trap cleanup EXIT
python3 -u "$here/serve.py" "$build/browser" "$PORT" >>"$out/server.log" 2>&1 &
server_pid=$!
for _ in $(seq 1 30); do
	curl -fsS --max-time 2 "$BASE/build-info.json" -o /dev/null 2>/dev/null && break
	kill -0 "$server_pid" 2>/dev/null || {
		echo 'static server exited' >&2
		exit 1
	}
	sleep 1
done
curl -fsS --max-time 2 "$BASE/build-info.json" -o "$out/build-info.json"
nice -n 10 "$BUNDLES/node-24.22.0/bin/node" "$here/minaweb-keyfile-check.mjs" 2>&1 |
	tee -a "$out/check.log"
