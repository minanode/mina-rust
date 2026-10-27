# Sourced by the archive end-to-end scripts (R43b.1). User-level installs:
# Postgres 16 and OCaml's 4.0.2 archive debs extracted with dpkg-deb -x (see
# README.md). E2E is the run directory (default: the worktree's .work/e2e).
PG_PREFIX=${PG_PREFIX:-$HOME/.local/opt/postgresql-16}
ARCHIVE_PREFIX=${ARCHIVE_PREFIX:-$HOME/.local/opt/mina-archive-4.0.2}
OCAML_PREFIX=${OCAML_PREFIX:-$HOME/.local/opt/mina-4.0.2}
PG_BIN=$PG_PREFIX/usr/lib/postgresql/16/bin
PG_PORT=${PG_PORT:-3445}
ARCHIVE_PORT=${ARCHIVE_PORT:-3446}
E2E=${E2E:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/.work/e2e}
ARCHIVE_URI=postgres://archive@127.0.0.1:$PG_PORT/archive
export MINA_PROFILE=${MINA_PROFILE:-mainnet}
export LD_LIBRARY_PATH=$PG_PREFIX/usr/lib/x86_64-linux-gnu:$OCAML_PREFIX/usr/lib/x86_64-linux-gnu
