#!/usr/bin/env bash
# Run OCaml's 4.0.2 archive process on ARCHIVE_PORT, writing to the e2e
# Postgres. Prints nothing; logs JSON to $E2E/archive.log. Its PID is $!.
set -euo pipefail
source "$(dirname "$0")/env.sh"
export TMPDIR=$E2E/tmp
mkdir -p "$TMPDIR"
exec "$ARCHIVE_PREFIX/usr/local/bin/mina-archive" run --postgres-uri "$ARCHIVE_URI" \
	--server-port "$ARCHIVE_PORT" --log-json "$@" >>"$E2E/archive.log" 2>&1
