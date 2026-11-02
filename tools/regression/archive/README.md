# Archive compatibility with OCaml 4.0.4

The Rust live feeder uses `Send_archive_diff` v0, as the OCaml 4.0.4 daemon
still does. Its durable outbox retains acknowledged diffs for bounded replay. An
acknowledgement means the archive accepted the RPC; check PostgreSQL rows and
archive logs to establish that the transaction committed.

`pin.json` pins the official noble amd64 archive package, source commit
`c9e2eac5344da621828c1e96c074b8bdce665b51`, executable and SQL hashes. The 4.0.3
source schema is separately pinned for the migration regression. Historical
4.0.2 runs remain evidence for those binaries only.

## Install

Install PostgreSQL 16 and the archive runtime libraries (`libpq`, `libjemalloc`,
OpenSSL and libgomp). A user-level extraction of the distribution packages is
sufficient. Set `PG_PREFIX` and, if needed, `LD_LIBRARY_PATH` for that install.
Choose private installation and evidence directories, then run:

```bash
python3 tools/regression/archive/install.py \
  --prefix "$ARCHIVE_PREFIX" --scratch "$RUN/downloads"
```

The installer checks the package hash before extraction, verifies the installed
binary and SQL, and writes a provenance README. It refuses to overwrite an
existing prefix. `MINA_PROFILE` must identify the network; OCaml processes must
have a private `TMPDIR`. No daemon or database needs to be running to install.

## Upgrade an operator database

These are operator steps, not permission for a regression run to modify a live
archive. Back up the database and verify that the backup restores first. Stop
its archive writer and retain the node's outbox while changing the schema. Use
the **4.0.4 package's** `upgrade.sql`, including when the existing database
already has the 4.0.3 `bytea` columns:

```bash
psql -X -v ON_ERROR_STOP=1 "$ARCHIVE_DATABASE_URI" \
  -f "$ARCHIVE_PREFIX/etc/mina/archive/upgrade.sql"
```

Do not wrap the upstream script in an extra transaction. It uses small,
idempotent steps. It adds `genesis_accounts` and the three
`idx_user_commands_{fee_payer,source,receiver}_id` indexes; it leaves existing
`bytea` values intact. Confirm that the newest `migration_history` row has
migration version `0.0.3` and status `applied`, that all three indexes are
valid, and that both `token_symbols.value` and `zkapp_uris.value` are `bytea`.

The script's `4.0.0` / `5.0.0` header describes the schema's protocol eras, not
the archive package release. Its `upgrades: 4.0.2` baseline does not mean a
4.0.3-schema database should skip it. On large archives, the script documents
optional `CREATE INDEX CONCURRENTLY` commands to run beforehand; normal index
creation temporarily blocks writers.

Start the pinned 4.0.4 archive, resume delivery, and compare retained block
hashes against `blocks`, checking archive errors as well as outbox status.
Replay retained diffs using the private operator endpoint if necessary:

```bash
curl -X POST "$NODE_URL/archive/resend"
curl "$NODE_URL/archive/status"
```

Replay is idempotent by state hash. It cannot recover diffs outside retention;
use independently saved blocks and the archive import tools for that history.
For a fresh database, apply `create_schema.sql` with `ON_ERROR_STOP=1` instead
of `upgrade.sql`. A fresh schema need not have a migration-history row.

For a **4.0.2 text** database, separately establish the historical encoding
before migration. The default conversion uses the database encoding; set
`archive.value_encoding` only when evidence establishes a different original
encoding such as LATIN1. The 4.0.3-to-4.0.4 test does not establish how an older
operator database encoded its text. Downgrading to text cannot preserve every
new byte value: NUL and invalid UTF-8 can make rollback fail. Restore a verified
backup rather than promising a lossless downgrade.

## Historical import and genesis accounts

The new `Send_versioned_precomputed_block` v0 takes the original versioned JSON
as a binprot string and returns `unit Or_error`, unlike the live diff RPC's unit
response. It is used by OCaml's explicit historical import forwarding; the live
daemon still dispatches `Archive_lib.Rpc.t`.

Keep the Rust live feeder on its existing RPC. This change does not add Rust's
`archiveVersionedPrecomputedBlock` mutation or claim Berkeley historical replay.
Never relabel a Berkeley V3 block as Mesa V4: doing so changes its hashes. Use
the pinned OCaml archive import tooling for versioned history, with per-version
fixtures before declaring historical parity.

Creating `genesis_accounts` does not populate it. Accounts untouched by blocks
cannot be reconstructed from `accounts_accessed`. Populate each era from its
actual genesis runtime config using the pinned archive toolbox:

```bash
"$ARCHIVE_PREFIX/usr/local/bin/mina-archive-hardfork-toolbox" \
  populate-genesis-accounts --config-file "$GENESIS_CONFIG" \
  --postgres-uri "$ARCHIVE_DATABASE_URI"
```

The table is keyed by public key, token and genesis height, so an older era's
snapshot remains available. This is an explicit operator step, separate from
Rust's live breadcrumb feed.

Source:
[4.0.4 archive client](https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_lib/archive_client.ml),
[RPC definitions](https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/archive/lib/rpc.ml),
and
[upgrade script](https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/archive/upgrade.sql).

## Run the regression

Build the `mina-node-common` library tests using the repository build lock:

```bash
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
  tools/regression/lock.sh build -- \
  cargo test --release -p mina-node-common --lib --no-run
```

Use the executable path Cargo prints as `COMMON_TEST`. Supply a saved Mesa
`ArchiveRpc` payload as `PAYLOAD` and a new directory as `RUN/check`. The old
Berkeley breadcrumb fixture cannot be decoded as a Mesa diff.

```bash
python3 -B tools/regression/archive/check.py \
  --archive-prefix "$ARCHIVE_PREFIX" --pg-prefix "$PG_PREFIX" \
  --test-binary "$COMMON_TEST" --payload "$PAYLOAD" \
  --run-dir "$RUN/check" --pg-port 5440 --archive-port 4669
```

The check refuses mismatched archive/schema hashes and occupied ports, creates
its own PostgreSQL instance, and always stops its archive and database. It
checks fresh 4.0.4 and 4.0.3 plus `upgrade.sql`, migration reapplication,
byte-preserving migration, two genesis-era imports (17 accounts each), and
committed block/account rows through the real Rust outbox and RPC client. Each
path sends the original payload and five synthetic account snapshots: `ff`,
`00`, `610062`, `5c78` (literal backslash-x), and `c3a9` (UTF-8 é). These
deliberately changed account snapshots test storage; they are not
consensus-valid blocks. Expected bytes are literal inputs, and SQL comparison
uses `encode(value, 'hex')`. The check requires the named Rust test to run,
checks database commits after acknowledgement, rejects archive errors, and
writes `result.json` plus logs and expected rows.

## Live node delivery

The helper scripts require `E2E` to be an absolute private directory. Select
unused ports; defaults are PostgreSQL 5440 and archive RPC 4669. Set `MINA` to
the verified Rust node executable and `MINA_GENESIS_CONFIG` to the network's
runtime config. For a fresh-schema mainnet run:

```bash
export E2E="$RUN/live" PG_PORT=5440 ARCHIVE_PORT=4669
A=tools/regression/archive
"$A/postgres.sh" start
"$A/archive.sh" & archive_pid=$!
"$A/node.sh" 4660 9770 & node_pid=$!
curl -s http://127.0.0.1:4660/archive/status
"$A/psql.sh" -c 'select count(*), min(height), max(height) from blocks'
```

The node helper takes the shared node lock. Stop only the PIDs started by this
run, then stop its database with `postgres.sh stop`. The outbox survives in the
node data directory's `archive-outbox` subdirectory. Compare all retained hashes
and parent links with SQL, not just a count. Repeat with a 4.0.3-schema database
after applying the pinned upgrade. Do not run `postgres.sh start` against an
operator database.
