# Sourced by the archive end-to-end scripts (R43b.1). User-level installs:
# Postgres 16 and OCaml's pinned 4.0.4 archive package (see README.md).
# E2E must name a private run directory; never point it at a live database.
PG_PREFIX=${PG_PREFIX:-$HOME/.local/opt/postgresql-16}
ARCHIVE_PREFIX=${ARCHIVE_PREFIX:-$HOME/.local/opt/mina-archive-4.0.4}
OCAML_PREFIX=${OCAML_PREFIX:-$HOME/.local/opt/mina-4.0.2}
PG_BIN=$PG_PREFIX/usr/lib/postgresql/16/bin
PG_PORT=${PG_PORT:-5440}
ARCHIVE_PORT=${ARCHIVE_PORT:-4669}
E2E=${E2E:?set E2E to an absolute private run directory}
ARCHIVE_DB=${ARCHIVE_DB:-archive}
ARCHIVE_URI=postgres://archive@127.0.0.1:$PG_PORT/$ARCHIVE_DB
export MINA_PROFILE=${MINA_PROFILE:-mainnet}
export LD_LIBRARY_PATH=$PG_PREFIX/usr/lib/x86_64-linux-gnu:$OCAML_PREFIX/usr/lib/x86_64-linux-gnu
