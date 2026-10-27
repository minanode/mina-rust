#!/usr/bin/env bash
# Run a command in this experiment's private user/network namespace.
set -euo pipefail
work=$1
shift
[[ $# -gt 0 ]] || { echo "usage: enter.sh RUN_DIR COMMAND [ARGS...]" >&2; exit 2; }
pid=$(cat "$work/namespace.pid")
expected=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["netns"])' "$work/namespace.json")
[[ $(readlink "/proc/$pid/ns/net") == "$expected" ]] || { echo "namespace no longer exists" >&2; exit 1; }
# A single-UID user namespace cannot chown extracted archives to host UID 1000.
export TAR_OPTIONS=--no-same-owner
exec nsenter --target "$pid" --user --net --preserve-credentials -- "$@"
