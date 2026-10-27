---
sidebar_position: 8
title: Node Management
description: Essential commands and tools for managing your Mina Rust node
---

# Node Management

This guide covers essential commands and tools for managing your Mina Rust node,
including version verification, monitoring, and troubleshooting.

## Checking Build Information

It's important to verify the version and build information of your node to
ensure you're running the correct version and to help with troubleshooting.

### Using Docker

To check the build information of your Docker-based node:

```bash
# Get build information
docker run --rm o1labs/mina-rust:latest build-info
```

Example output:

```
Version:       0341fff
Build time:    2025-09-10T18:11:40.953050700Z
Commit SHA:    0341fffedba4900371504ad4b30853674960a209
Commit time:   2025-09-10T19:30:31.000000000+02:00
Commit branch: release/v0.17.0
Rustc channel: stable
Rustc version: 1.84.1
```

For a specific version:

```bash
docker run --rm o1labs/mina-rust:v0.18.0 build-info
```

### Using Native Binary

If you've built from source or are running the binary directly:

```bash
# Get build information
mina build-info

# Or with the full path if not in PATH
./target/release/mina build-info
```

### Understanding Build Information

The build info provides critical details:

- **Version**: Short commit hash identifying the exact code version
- **Build time**: When the binary was compiled
- **Commit SHA**: Full Git commit hash for precise version tracking
- **Commit time**: When the source code commit was made
- **Commit branch**: The Git branch used for the build
- **Rustc channel**: Rust compiler channel (stable/beta/nightly)
- **Rustc version**: Exact Rust compiler version used

### Verifying Your Installation

Always verify your node version after installation or updates:

```bash
# For Docker users
docker run --rm o1labs/mina-rust:latest build-info | grep "Version\|branch"

# For native binary users
mina build-info | grep "Version\|branch"
```

## Monitoring Your Node

### Read-only daemon status

Inspect a running Rust node with one command:

```bash
mina node status --node http://127.0.0.1:3000
mina node status --node http://127.0.0.1:3000 --json
```

This uses only GET requests to `/status`, `/build_env`, and
`/stats/actions?id=latest`. The reported network and build belong to the remote
node; the CLI's global `--network` does not select the server. The command does
not start a node, submit transactions, or change configuration.

For a server where only `/status` reads are permitted, add `--status-only`. This
sends exactly one GET and leaves remote build/verification statistics
unavailable. The periodic watcher accepts the same flag. No `/state` or GraphQL
request is made in either mode.

The report includes network and chain ID, sync status/phase, applied best-tip
height and age, root height, connected peers by transport, remote build commit,
uptime, P2P allocation estimate, and recent block/work verification action
successes and failures. Libp2p peers are usually OCaml peers, but transport does
not prove the peer's implementation. Memory is **P2P allocation only**, not
process RSS. Verification counts cover actions **since the latest best-tip
change**, not lifetime totals or a fixed time window; work counts are batches,
not individual proofs. Missing metrics are `unavailable` in text and `null` in
JSON, never invented zeroes.

Older nodes may lack timestamp, root or uptime fields. For those responses, the
command can use a consistent timestamp advertised by connected peers for the
exact local tip hash, and derive root height from the contiguous applied chain's
size. Both sources are labelled in the output. Uptime remains unavailable.
Unknown chain IDs have no inferred network name. The tip's block timestamp is
used for age, never the last sync event's time.

### Health checks and periodic alerts

```bash
mina node status --node http://127.0.0.1:3000 --check \
  --max-tip-age 1800 --min-peers 1
```

`--check` prints one `OK:` or `UNHEALTHY:` line. Exit codes:

- `0`: the check passed (or a normal status read succeeded without `--check`).
- `1`: a required request/response failed, or a `--check` rule failed.
- `2`: invalid command-line arguments.

A health check requires `Synced`, an available tip timestamp at or before the
client's clock, tip age no greater than `--max-tip-age` **seconds**, and at
least `--min-peers` **connected** peers. Defaults are 1800 seconds and one peer.
Bootstrapping is unhealthy until those conditions hold. Future timestamps fail
with a clock warning. Optional build/statistics failures appear as warnings and
do not fail an otherwise healthy check. `--json --check` preserves structured
output, including `healthy`, `reasons`, `warnings`, and `schema_version`.
`--timeout` bounds each of the three requests (default 5 seconds).

For continuous tip-age and log-silence alerts, run this script from the
checkout:

```bash
python3 tools/regression/watch-status.py \
  --mina ./target/release/mina --url http://127.0.0.1:3000 \
  --interval 60 --max-tip-age 1800 --min-peers 1 \
  --log-file /path/to/node.log --max-log-silence 15 \
  >> /path/to/status-alerts.log 2>&1
```

The script logs UTC `ALERT` lines on each failing poll and `RECOVERED` when all
checks pass again. `--interval` is seconds between completed checks;
`--max-log-silence` is **minutes**. It checks log metadata independently of HTTP
success and never reads log contents, signals the node, or restarts it. Use a
separate alert log: writing alerts into the monitored node log would mask
silence. At the default level the node logs a status line at least every 10
minutes, so a silence of 15 minutes means it is stuck. HTTP requests are logged
only at `debug`; combine this check with tip age.

The first sample uses a nonempty file's modification time as a proxy for prior
activity. Subsequent samples require size growth; touching the file does not
reset the timer. Empty/missing files, future mtimes, and stale rotated files
alert. A new nonempty rotated file uses its mtime. Timing after the initial
sample uses a monotonic clock. `--once` supports a one-shot check with exit
`0`/`1`; repeated cron invocations of `--once` only have the mtime proxy, so use
the continuous mode for observed growth.

Tip age detects a stalled node, but does not compare heights with a public
reference or establish canonical-chain agreement. Long network-wide block gaps
can alert too. Choose thresholds for the network's block cadence and operator
response time; no watcher is installed automatically.

### Logs and log levels

The node logs to the console and to `mina.log` in its working directory.

At the default level, `info`, it logs what an operator follows, about a line per
event:

- `NodeStart`: the network, version, ports, working directory, and whether the
  frontier store, block producer, SNARK worker and log file are on;
- the sync phases (`TransitionFrontierSyncLedgerStakingSuccess` and so on), and
  `NodeStatus` lines with the progress and the peer count, at most once a minute
  and at least every 10 minutes;
- `NodeSynced` when the node first syncs and when it syncs again, and a
  `NodeLostSync` warning when it has not been synced for a minute;
- `TransitionFrontierSynced` with each new best tip's height, hash and peers;
- the blocks it produces (`BlockProducerWonSlot`, `BlockProducerBlockProduced`,
  `BlockProducerBlockInjected`, `BlockProducerBlockIntegrated`), and
  `SnarkWorkSold` when a block includes its SNARK work;
- warnings and errors.

A synced devnet node writes about 20 to 40 KB an hour, under 1 MB a day at this
level, and logs at least a line every 10 minutes. `--verbosity debug` (or
`--log-level debug`, or `MINA_VERBOSITY=debug`) brings back the detail the
`info` level showed in earlier builds (the SNARK pool, catch-up requests,
gossip, dials, block applications, HTTP requests and so on), about 6 MB an hour
on devnet. `--verbosity trace` logs every action of the node's state machine, as
`debug` did in earlier builds: about 220 MB an hour, and 1.5 GB an hour while it
bootstraps, too much to keep for long.

`ProducerBlockQueuedForGossip` and frontier restore details are at DEBUG. Run
with `--verbosity debug` to see those events when investigating a produced block
or a restore.

If the log directory cannot be used, the node warns
`file logging disabled: <reason>` and continues with console logging. A rotation
failure warns once and retries at most once a minute while preserving the
current file when possible.

`mina.log` is rotated by size: at `--log-file-size-mb` (default 20) it becomes
`mina.log.1`, the older files shift up one, and only `--log-file-count` files
(default 10) are kept, so the log files take at most 200 MB. `--log-path DIR`
puts them in another directory, and `--disable-filesystem-logging` turns the
file off: the console is then the only log. Earlier builds wrote a file a day,
`mina.log.YYYY-MM-DD`, and never deleted them; the node leaves those files
alone, so delete them once you no longer need them.

To report a problem, attach `mina.log` and `mina.log.1`; run the node with
`--verbosity debug` if you can reproduce it.

### View Logs

For Docker Compose setups:

```bash
# View all logs
docker compose logs -f

# View only node logs
docker compose logs -f mina-rust-node

# View last 100 lines
docker compose logs --tail=100 mina-rust-node
```

For Docker containers:

```bash
# View container logs
docker logs -f mina-rust-node

# View with timestamps
docker logs -f -t mina-rust-node
```

### Check Node Status

```bash
# Check if containers are running
docker compose ps

# Or for individual containers
docker ps | grep mina
```

### Resource Usage

Monitor CPU and memory usage:

```bash
# Docker stats
docker stats mina-rust-node

# System resources
htop
# or
top
```

## Common Management Tasks

### Restart Your Node

```bash
# Using Docker Compose
docker compose restart

# Or stop and start
docker compose down
docker compose up -d
```

### Update Your Node

```bash
# Pull latest images
docker compose pull

# Restart with new images
docker compose up -d --force-recreate
```

### Backup Configuration

Always backup your keys and configuration:

```bash
# Backup producer key (if running block producer)
cp -r mina-workdir/producer-key ~/mina-backup/

# Backup entire working directory
tar -czf mina-backup-$(date +%Y%m%d).tar.gz mina-workdir/
```

### Managing File Permissions

Docker containers often run as root, creating files owned by root on your host:

```bash
# Check file ownership
ls -la mina-workdir/

# Fix ownership for all files in mina-workdir
sudo chown -R $(id -u):$(id -g) mina-workdir/

# Set appropriate permissions for the producer key
chmod 600 mina-workdir/producer-key
```

<!-- prettier-ignore-start -->

:::tip

Best Practice Always fix file ownership after Docker operations to ensure your
local user can properly access, backup, and manage the files. This is especially
important for sensitive files like producer keys.

:::

<!-- prettier-ignore-stop -->

## Troubleshooting

### Node Won't Start

1. Check build info to verify correct version
2. Review logs for errors:
   ```bash
   docker compose logs --tail=100
   ```
3. Verify ports are available:
   ```bash
   netstat -tuln | grep -E "8302|3000|8070"
   ```

### Missing .env File Error

If you see an error like "env file .env not found", Docker Compose needs
environment variables:

**For regular node and block producer:**

```bash
# Quick fix: Create an empty .env file (has defaults)
touch .env

# Or create with custom settings
cat > .env << EOF
MINA_RUST_TAG=latest
MINA_FRONTEND_TAG=latest
MINA_LIBP2P_PORT=8302
MINA_LIBP2P_EXTERNAL_IP=
MINA_PRIVKEY_PASS=
COINBASE_RECEIVER=
EOF
```

**For archive node (required):**

```bash
# Archive node requires specific database settings
cat > .env << EOF
POSTGRES_PASSWORD=mina
PG_PORT=5432
PG_DB=archive
MINA_RUST_TAG=latest
EOF
```

Regular node docker-compose files have default values, but the archive node
docker-compose file requires the `.env` file with PostgreSQL configuration.

### Connection Issues

1. Verify external IP configuration
2. Check firewall rules
3. Test connectivity:
   ```bash
   telnet <external-ip> 8302
   ```

### Performance Issues

1. Check system resources:
   ```bash
   free -h
   df -h
   docker stats
   ```
2. Review node logs for warnings
3. Consider adjusting Docker resource limits

## Best Practices

1. **Regular Version Checks**: Always verify your node version after updates
2. **Log Monitoring**: Set up log rotation and monitoring
3. **Backup Strategy**: Regular backups of keys and configuration
4. **Update Schedule**: Plan updates during low-activity periods
5. **Version Documentation**: Keep track of which versions you're running

## Next Steps

- [Network Configuration](network-configuration) - Configure network settings
- [Block Producer](block-producer) - Set up block production
- [Archive Node](archive-node) - Run an archive node
