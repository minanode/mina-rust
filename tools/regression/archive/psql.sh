#!/usr/bin/env bash
# psql on the e2e archive database.
source "$(dirname "$0")/env.sh"
exec "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PG_PORT" -U archive "$ARCHIVE_DB" -At "$@"
