# Fresh sync demo

Install the verified release using the site's
[install guide](https://minanode.org/docs/install/). Put `mina` on `PATH`, or
pass its path. Run these scripts from a checkout of the release being measured.
Keep `fresh-sync.sh` and `sync-run.py` together when copying them. They need
Bash and Python 3 on a Unix host, with no Python packages.

```sh
tools/demo/fresh-sync.sh --network devnet --binary ~/.local/bin/mina --keep
```

Open the printed dashboard URL. The script prints elapsed time while the node
starts, then prints each observed phase transition and one result line at
`Synced`. `--keep` waits for Ctrl-C after the result. Without it the script
stops immediately after measuring. Both modes stop this node and remove its
fresh data directory on exit.

Use `--port 3010 --libp2p-port 8312` when the default ports are occupied.
`--network mainnet` selects mainnet. `--timeout 900` sets the sync deadline in
seconds. `--output demo-runs/run-1` retains logs, status samples and result JSON
in a new directory. No node data or identity keys are retained there.

For Docker, pull the documented image first, then use:

```sh
docker pull ghcr.io/minanode/mina-rust:0.20.0
tools/demo/fresh-sync.sh --docker ghcr.io/minanode/mina-rust:0.20.0 --keep
```

The runner resolves the image to its local ID before starting. It follows the
repository's compose conventions: caller UID/GID, fresh data mounted at `/data`,
HTTP published on loopback, and a separate libp2p port. Image download is
outside the timer. See [Docker releases](../../docs/release/DOCKER.md). Docker
is not used by the development-machine measurements. Cleanup always requests
`docker stop`, then `docker rm`, for this run's named container, even if the
attached client has exited. It then uses `docker inspect` to confirm the
container is stopped or absent before removing its data. If inspection cannot
confirm shutdown, the script fails and retains the data directory with the
container name and reason in its error. Restore access to Docker and stop that
container before removing its data.

## Repeated measurements

```sh
python3 tools/demo/sync-matrix.py --runs 5 --network devnet \
  --binary ~/.local/bin/mina --output sync-results/devnet
python3 tools/demo/sync-matrix.py --runs 5 --network devnet \
  --binary ~/.local/bin/mina --cpus 0-3 --memory-max 8G \
  --output sync-results/devnet-small
```

The harness runs one node at a time and reuses the specified ports after
stopping it. It writes `results.jsonl`, `summary.md`, machine and constraint
metadata, and per-run evidence. Output directories must not already exist.
Failed runs stay in JSONL and in the summary, but do not enter successful-run
percentiles.

CPU pinning needs Linux `taskset`. The memory cap needs a working systemd user
manager with delegated memory control. `8G` means 8 GiB. Swap is disabled for
capped runs. `--systemd` runs uncapped trials in named user services too. These
wrappers support native binaries; they refuse Docker because the Docker daemon
would launch the node outside the constrained unit.

On a shared machine, pick free ports, put `--temp-root` on a disk with room, and
set `--min-free-gib` to the floor you need. To queue runs behind other jobs,
pass a lock command with `--node-lock`. The lock wait and systemd startup are
outside the sync timer. The runner creates the `--temp-root` parent if it is
missing. That parent remains after cleanup; only the temporary node directory is
removed. The disk floor applies to both node data and retained evidence,
including while waiting with `--keep`.

## What the measurements mean

Time to Synced is monotonic elapsed time from just before node launch until the
first `/status` response whose phase is `Synced` and whose best tip has a
height. Polling is every 0.5 seconds plus request latency. Each HTTP request has
a one-second total deadline for headers, body and JSON parsing, enforced by a
Unix timer. Status requests also stay within the remaining sync deadline.
Responses are limited to 4 MiB. The runner checks the sync deadline again before
accepting Synced. A trickling response cannot indefinitely delay the
elapsed-time display or disk checks.

Phase durations use observed transitions. The raw `sync.time` timestamp from
`/status` is saved alongside the observation time. It describes the current
internal sync state, so it is not always the start of the displayed phase.
Unobserved phases are unavailable, not zero. Durations for repeated visits to a
phase are added. Time before the first response is reported as `Starting`.

Peak RSS is the Linux node process's `VmHWM` through the first Synced response.
It excludes the harness and memory used after Synced with `--keep`. It is not
cgroup memory, which also accounts for page cache. On hosts without readable
node `/proc` data, including Docker Desktop, RSS is reported as unavailable. The
median and nearest-rank p90 include only successful trials. With five trials,
p90 is the maximum. The sample count is printed for each metric.

Each trial has a fresh work directory and HOME, including verifier and ledger
caches. No producer, snarker or archive settings are inherited from the shell.
The operating system page cache is not flushed. These are live peer-network
measurements, so peers, chain height, host load and connection conditions vary.
Version, full commit and binary SHA-256 (or Docker image ID) identify the
measured build. Do not substitute the checkout's commit for the binary's commit.

The site claim remains a hidden TODO draft until RC6 is measured and reviewed.
