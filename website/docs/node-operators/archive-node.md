# Run an archive node

## OCaml 4.0.4 compatibility

Use the archive binary and SQL from the same release. The regression pin is
OCaml 4.0.4, commit `c9e2eac5344da621828c1e96c074b8bdce665b51`; package and
schema hashes are recorded in `tools/regression/archive/pin.json`.

Before upgrading an existing archive, back up the database, verify the restore,
and stop its archive writer. Retain the Rust node's archive outbox. Even a 4.0.3
database with `bytea` columns needs the 4.0.4 package's migration:

```bash
psql -X -v ON_ERROR_STOP=1 "$ARCHIVE_DATABASE_URI" \
  -f "$ARCHIVE_PREFIX/etc/mina/archive/upgrade.sql"
```

The migration adds `genesis_accounts` and indexes on the fee payer, source and
receiver columns of `user_commands`. Check the newest `migration_history` row
for version `0.0.3` and status `applied`, and check that the indexes are valid.
The script's `4.0.0` / `5.0.0` labels are protocol eras, not package versions.
For a fresh database, use the matching `create_schema.sql` instead.

Restart the archive and compare stored block hashes with the retained outbox;
RPC acknowledgement alone does not prove database commitment. The
[database check](#check-the-archive-database) reads the archive's own counts of
its database. The private `POST /archive/resend` endpoint replays retained diffs
if needed: one block with `{"state_hash": "3N..."}`, or every retained diff with
`{"all": true}`. The request needs the `x-mina-archive-control` header and a
JSON body:

```bash
curl -X POST http://127.0.0.1:3000/archive/resend \
  -H 'x-mina-archive-control: 1' -H 'content-type: application/json' \
  -d '{"all": true}'
```

From a browser, only pages of this machine, the node's own dashboard at an IP
address and `--worker-control-origin` origins may send it. The dashboard's
Archive page shows the feed and replays blocks. The live feeder continues to use
`Send_archive_diff` v0, matching OCaml's daemon.

Historical versioned JSON import and genesis-ledger population are separate
operations. Creating `genesis_accounts` does not fill it: use the pinned
`mina-archive-hardfork-toolbox populate-genesis-accounts` command with each
era's actual runtime config. Rust's live feed does not reconstruct untouched
genesis accounts. See `tools/regression/archive/README.md` for the complete
recipe and regression command. A rollback to a text schema can fail on NUL or
invalid UTF-8 bytes; do not assume a lossless downgrade.

## Check the archive database

The OCaml archive process counts its own `blocks` table and serves the counts as
Prometheus gauges when you start it with `--metrics-port`. The Rust node reads
them from `MINA_ARCHIVE_METRICS_ADDRESS` every 60 seconds, with a 5-second
timeout and a 1 MiB limit on the answer. The node never connects to PostgreSQL
and needs no database password.

```bash
mina-archive run --postgres-uri "$ARCHIVE_DATABASE_URI" \
  --server-port 3086 --metrics-port 3087

MINA_ARCHIVE_ADDRESS=http://127.0.0.1:3086 \
MINA_ARCHIVE_METRICS_ADDRESS=http://127.0.0.1:3087 \
  mina node --archive-archiver-process
```

A bare address gets the path `/metrics`. The node refuses to start if the value
is not an `http` or `https` URL. The archive's metrics and RPC servers listen on
every network interface, and the archive has no option to choose one. Block both
ports at the firewall, or run the archive where only the node can reach them.

The node reads three gauges:

| Gauge                            | Meaning                                                                                                |
| -------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `Coda_Archive_max_block_height`  | The highest block height in `blocks`.                                                                  |
| `Coda_Archive_missing_blocks`    | Heights with no block row, from `max(lowest height, highest - width)` to the highest height.           |
| `Coda_Archive_unparented_blocks` | Blocks above the lowest height whose `parent_id` is null, because their parent is not in the database. |

The width is the archive's `--missing-blocks-width`, 2000 by default. The
archive recomputes the gauges every two slots, which is 3 minutes on Mesa.

`GET /archive/status` reports the check under `database`, and the dashboard's
Archive page shows it on the **Archive database** card. The `verdict` is one of
these values:

- `complete` means no missing heights, no blocks without a parent, and a highest
  stored height at least as high as the highest block the node had delivered 6
  minutes earlier.
- `incomplete` means at least one missing height or block without a parent.
- `behind` means the highest stored height is below a block the node delivered 6
  minutes ago or more. The archive acknowledged a block it did not write, or its
  gauges stopped refreshing.
- `not_checked` means `MINA_ARCHIVE_METRICS_ADDRESS` is not set, or the first
  read has not finished.
- `unavailable` means the last read failed, or the answer has no archive gauges.
  `last_error` says why. The last good counts stay, with their time.

The 6 minutes are two archive refreshes: four slots of the node's network. The
node also reports `delivered_height`, the highest block the archive acknowledged
since the node started, and `stored_tip_lag`, that height minus the highest
stored height.

```bash
curl -s http://127.0.0.1:3000/archive/status | jq .database
```

If the verdict is `incomplete`, resend the retained blocks with `{"all": true}`,
as above. The archive adds the blocks it lacks among the last 870 heights.
Import older blocks from precomputed block files with
`mina-archive-blocks --precomputed`.

The check has these limits:

- It gives counts, not heights. Run `mina-missing-blocks-auditor` against the
  database to list them.
- A change in the database shows in the status within about 4 minutes: up to 3
  minutes until the archive's next refresh, then up to 1 minute until the node's
  next read.
- Missing heights cover only the archive's window. An older gap does not count.
  Blocks without a parent cover the whole database.
- The Rust node sends only its best chain. Blocks of other forks reach the
  database only from other senders. A height with any block is not missing.

## Devnet deployment

This guide is intended for setting up archive nodes on **Mina Devnet** only. Do
not use this guide for Mina Mainnet until necessary security audits are
complete.

## What is an Archive Node?

An archive node is a specialized Mina node that stores the complete blockchain
history in a structured database. Unlike regular nodes that only maintain recent
state for consensus, archive nodes:

- **Store all blocks**: Maintains every block from genesis to the current tip
- **Preserve transaction history**: Keeps a complete record of all transactions
- **Provide historical queries**: Enables querying of past blockchain states
- **Support analytics**: Facilitates blockchain analysis and data exploration
- **Enable compliance**: Helps meet regulatory requirements for data retention

### Synchronization Behavior

<!-- prettier-ignore-start -->

:::note Current Limitation

Archive nodes currently sync and store blocks from the point when they are
started. There is no built-in option to automatically replay and store the
complete blockchain history from genesis block.

To obtain complete historical data, you would need to:

1. Start the archive node from genesis (when the network was launched)
2. Import historical data from another archive node's database
3. Use existing archive services that have been running since genesis

:::

<!-- prettier-ignore-stop -->

The archive node consists of three components:

1. **Mina node**: Syncs with the network and receives new blocks
2. **Archiver process**: Processes blocks and stores them in the database
3. **PostgreSQL database**: Stores the structured blockchain data

---

## Prerequisites

Ensure Docker and Docker Compose are installed on your system -
[Docker Installation Guide](../appendix/docker-installation)

## Using Docker Compose

1. **Download the Docker Compose File**

   Create a directory for your archive node and download the docker-compose
   file:

   ```bash
   # Create a directory for your archive node
   mkdir mina-archive-node && cd mina-archive-node

   # Download the archive node docker-compose file (choose one method)
   # Using wget:
   wget https://raw.githubusercontent.com/o1-labs/mina-rust/v0.18.0/docker-compose.archive.devnet.yml

   # Or using curl:
   curl -O https://raw.githubusercontent.com/o1-labs/mina-rust/v0.18.0/docker-compose.archive.devnet.yml

   # Create required .env file with PostgreSQL settings
   cat > .env << EOF
   POSTGRES_PASSWORD=mina
   PG_PORT=5432
   PG_DB=archive
   MINA_RUST_TAG=latest
   EOF
   ```

   For the latest development version, replace `v0.18.0` with `develop` in the
   URL.

   <!-- prettier-ignore-start -->

   :::warning Required Configuration

   The archive node requires a `.env` file with PostgreSQL database settings.
   The example above provides the minimum required configuration. You can
   customize the database password and other settings as needed.

   :::

   <!-- prettier-ignore-stop -->

2. **Launch Archive Node**

   The archive node setup includes a PostgreSQL database, the archiver process,
   and the Mina Rust node. The archiver process stores blocks in the database by
   receiving them from the Mina Rust node.

   ```bash
   docker compose -f docker-compose.archive.devnet.yml up -d --pull always
   ```

   **Configuration Options:**
   - `MINA_RUST_TAG` - Docker image tag for the mina-rust node (default:
     `latest`)
   - `POSTGRES_PASSWORD` - Database password for PostgreSQL
   - `PG_PORT` - PostgreSQL port (default: `5432`)
   - `PG_DB` - Database name (default: `archive`)

   **Examples with different versions:**

   ```bash
   # Use specific version (recommended for production)
   env MINA_RUST_TAG="v1.4.2" \
   docker compose -f docker-compose.archive.devnet.yml up -d --pull always

   # Use development version (latest features, may be unstable)
   env MINA_RUST_TAG="develop" \
   docker compose -f docker-compose.archive.devnet.yml up -d --pull always
   ```

3. **Monitor the Archive Node**

   The archive node components will be accessible at:
   - **Archive Service**: localhost:3086 (RPC protocol, not HTTP)
   - **Node GraphQL API**: http://localhost:3000/graphql
   - **PostgreSQL Database**: localhost:5432

## Analyzing and Querying Archive Data

Once your archive node is running, you can analyze the stored blockchain data
through multiple interfaces:

### Database Access

The archive node stores data in a PostgreSQL database with over 45 tables
containing complete blockchain history. You can:

- **Direct SQL queries** - Connect directly to PostgreSQL for complex analysis
- **GraphQL API** - Use the node's GraphQL endpoint for programmatic access
- **Pre-built queries** - Use existing SQL templates for common operations

### Quick Database Test

```bash
# Connect to the database
docker exec postgres-mina-rust psql -U postgres -d archive -c "\dt"

# Check recent blocks
docker exec postgres-mina-rust psql -U postgres -d archive -c "SELECT COUNT(*) as total_blocks FROM blocks;"
```

### Documentation for Developers

For comprehensive guides on querying and analyzing archive data:

- **[Archive Database Queries](../developers/api-and-data/archive-database-queries)** -
  Complete SQL reference, schema documentation, and analysis examples
- **[GraphQL API Reference](../developers/api-and-data/graphql-api)** - Full
  GraphQL endpoint documentation and query examples

These developer guides include:

- Complete database schema and table relationships
- Working SQL queries for blockchain analysis
- GraphQL queries for real-time data access
- Performance optimization techniques
- Data export and backup procedures

### Common Use Cases

1. **Compliance and Auditing**: Track all transactions for regulatory compliance
2. **Analytics Dashboards**: Build real-time blockchain analytics
3. **Research**: Analyze network behavior, transaction patterns, and economics
4. **Block Explorers**: Power blockchain explorer websites
5. **Tax Reporting**: Generate transaction history for tax purposes
6. **Network Monitoring**: Track network health and validator performance

## Node Parameters Reference

For a complete list of all available archive node parameters and configuration
options, see the
[Mina Rust API Documentation](https://o1-labs.github.io/mina-rust/api-docs/mina_cli/commands/node/struct.NodeArgs.html).
This includes detailed descriptions of:

- **Archive configuration flags**: `--archive-archiver-process`,
  `--archive-local-storage`, `--archive-gcp-storage`, `--archive-aws-storage`
- **Network settings**: `--libp2p-*`, `--network`, `--port`
- **Logging and debugging options**: `--verbosity`, `--log-*`
- **Performance tuning parameters**: Connection limits, timeouts, etc.
- **Security and validation settings**: Key management, validation options

You can also view available parameters by running:

```bash
# View all node subcommand options
mina node --help

# View specific archive-related options
mina node --help | grep -A 10 -B 2 archive
```

The source code documentation can be found in
[`cli/src/commands/node/mod.rs`](https://github.com/o1-labs/mina-rust/blob/develop/cli/src/commands/node/mod.rs)
which contains comprehensive examples and parameter descriptions for all archive
node configurations.

## Using Make Command

As an alternative to Docker Compose, you can run the archive node directly using
the Makefile target. This method requires building from source.

### Prerequisites

- Rust toolchain installed
- Git repository cloned and accessible
- PostgreSQL database running and configured

### Archive Mode Configuration

Mina Rust supports multiple archive modes that can be run simultaneously for
redundancy. Each mode stores blockchain data in a different location or format.

#### Available Archive Modes

##### 1. Archiver Process (`--archive-archiver-process`)

- **Purpose**: Stores blocks in a PostgreSQL database
- **Method**: Receives blocks directly from the Mina Rust node via RPC
- **Use case**: Primary archive method for database queries and analysis

**Required environment variables:**

- `MINA_ARCHIVE_ADDRESS`: Network address for the archiver service (e.g.,
  `http://localhost:3086`)

**Optional environment variables:**

- `MINA_ARCHIVE_METRICS_ADDRESS`: The archive's `--metrics-port` URL, for the
  [database check](#check-the-archive-database) (e.g., `http://localhost:3087`)

##### 2. Local Storage (`--archive-local-storage`)

- **Purpose**: Stores blocks in the local filesystem
- **Method**: Saves precomputed blocks as files on disk
- **Use case**: Local backup and offline analysis

**Optional environment variables:**

- `MINA_ARCHIVE_LOCAL_STORAGE_PATH`: Custom storage path
  - Default: `~/.mina/archive-precomputed`

##### 3. GCP Storage (`--archive-gcp-storage`)

- **Purpose**: Uploads blocks to Google Cloud Platform bucket
- **Method**: Stores precomputed blocks in GCP Cloud Storage
- **Use case**: Cloud backup with GCP integration

**Required environment variables:**

- `GCP_CREDENTIALS_JSON`: Service account credentials JSON
- `GCP_BUCKET_NAME`: Target GCP storage bucket name

##### 4. AWS Storage (`--archive-aws-storage`)

- **Purpose**: Uploads blocks to AWS S3 bucket
- **Method**: Stores precomputed blocks in Amazon S3
- **Use case**: Cloud backup with AWS integration

**Required environment variables:**

- `AWS_ACCESS_KEY_ID`: IAM user access key
- `AWS_SECRET_ACCESS_KEY`: IAM user secret key
- `AWS_DEFAULT_REGION`: AWS region (e.g., `us-west-2`)
- `MINA_AWS_BUCKET_NAME`: Target S3 bucket name
- `AWS_SESSION_TOKEN`: Temporary session token (for temporary credentials)

### Setup and Run

1. **Run Archive Node with Archiver Process**

   For devnet (default):

   ```bash
   MINA_ARCHIVE_ADDRESS="http://localhost:3086" \
   make run-node NETWORK=devnet -- --archive-archiver-process
   ```

   For mainnet (when supported):

   ```bash
   MINA_ARCHIVE_ADDRESS="http://localhost:3086" \
   make run-node NETWORK=mainnet -- --archive-archiver-process
   ```

2. **Run with Multiple Archive Modes (Redundancy)**

   You can combine multiple archive modes for redundancy:

   ```bash
   # Archive to both database and local storage
   MINA_ARCHIVE_ADDRESS="http://localhost:3086" \
   MINA_ARCHIVE_LOCAL_STORAGE_PATH="/path/to/archive" \
   make run-node NETWORK=devnet -- \
     --archive-archiver-process \
     --archive-local-storage
   ```

   ```bash
   # Archive to database, local storage, and AWS S3
   MINA_ARCHIVE_ADDRESS="http://localhost:3086" \
   MINA_ARCHIVE_LOCAL_STORAGE_PATH="/path/to/archive" \
   AWS_ACCESS_KEY_ID="your-access-key" \
   AWS_SECRET_ACCESS_KEY="your-secret-key" \
   AWS_DEFAULT_REGION="us-west-2" \
   MINA_AWS_BUCKET_NAME="your-bucket-name" \
   make run-node NETWORK=devnet -- \
     --archive-archiver-process \
     --archive-local-storage \
     --archive-aws-storage
   ```

3. **Monitor the Node**

   The node will start and listen on port 3000. You can monitor its status by
   checking the console output or connecting a frontend dashboard.

### Access Logs

Logs are stored in the working directory as `mina.log`, rotated to `mina.log.1`
and so on, at most 200 MB by default. See
[Logs and log levels](node-management#logs-and-log-levels).

### Provide Feedback

Collect logs and report issues on the
[rust-node-testing](https://discord.com/channels/484437221055922177/1290662938734231552)
Discord channel. Include reproduction steps if possible.
