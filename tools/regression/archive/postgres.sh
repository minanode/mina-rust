#!/usr/bin/env bash
# Create (first run) and start, or stop, the e2e Postgres with the archive
# schema: postgres.sh start|stop
set -euo pipefail
source "$(dirname "$0")/env.sh"
mkdir -p "$E2E"
case ${1:-start} in
start)
	if [[ ! -d $E2E/pgdata ]]; then
		"$PG_BIN/initdb" -D "$E2E/pgdata" -U archive --auth=trust >"$E2E/initdb.log"
		fresh=1
	fi
	"$PG_BIN/pg_ctl" -D "$E2E/pgdata" -l "$E2E/postgres.log" \
		-o "-p $PG_PORT -k $E2E -c listen_addresses=127.0.0.1" start
	if [[ -n ${fresh:-} ]]; then
		"$PG_BIN/createdb" -h 127.0.0.1 -p "$PG_PORT" -U archive archive
		"$PG_BIN/psql" -q -h 127.0.0.1 -p "$PG_PORT" -U archive archive \
			-f "$ARCHIVE_PREFIX/etc/mina/archive/create_schema.sql"
	fi
	;;
stop) "$PG_BIN/pg_ctl" -D "$E2E/pgdata" stop ;;
*) echo "usage: $0 start|stop" >&2 && exit 2 ;;
esac
