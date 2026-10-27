---
title: Mesa operator quick start
sidebar_position: 1
description: Pinned source build, local dashboard and status checks for Mesa.
---

# Mesa operator quick start

**Superseded for 0.20 source builds:** use the minanode.org
[Build from source](https://minanode.org/docs/build-from-source/) guide. This
page remains a record of the older mainnet rehearsal workflow.

For the planned 0.20.0 Linux devnet archive, see the
[release guide](release-020.md). This page builds the source and focuses on a
mainnet follower rehearsal; its 32 GB RAM and 60 GB disk budget is for that
source workflow, not the release binary's runtime minimum.

Do not use the older Berkeley Docker images or upstream release binaries for
this Mesa recipe. This community fork is experimental; syncing is not a claim of
complete historical validation or production readiness. This page runs a
non-producing native node and its monitoring dashboard, not a browser node or a
block producer.

## Prerequisites

Use x86-64 Linux (Ubuntu 24.04 is the rehearsal platform), Bash, a reliable
Internet connection, and an accurate system clock. Budget **32 GB RAM and 60 GB
free disk** for building and running; these are provisional planning allowances,
not measured minimums. Keep extra space for growing logs. A small Mina proof
does not imply a tiny ledger, source build or resident node. Limit Cargo to
eight jobs; on a smaller machine use fewer and expect a slower build.

Have an administrator install `build-essential`, `pkg-config`, `libssl-dev`,
`clang`, `cmake`, `protobuf-compiler`, `git`, `curl`, `ca-certificates`,
`python3`, `time`, and `util-linux`. For workspace tests also install
`libpcap-dev`; replay and genesis tooling need `zstd`, `openssl`, `tar` and
`iproute2`. The [developer setup scripts](../developers/getting-started.mdx)
maintain the full list. The instructions below do not install system packages.
Install Rust using [rustup](https://rustup.rs/) and Node.js 24 with npm before
beginning. The repository's `rust-toolchain.toml` selects **Rust 1.92**; native
builds do not need the nightly toolchain used for formatting and WebAssembly.

**Before starting a node, protect its HTTP port.** At this pin the node binds
HTTP to **all IPv4 interfaces (`0.0.0.0`)**, with no CLI bind-address option.
GraphQL shares that HTTP listener at `/graphql`; there is no separate GraphQL
port in this recipe. Use host/cloud firewall rules that deny external access to
the HTTP port, or a private host behind an authenticated proxy whose backend
cannot be reached directly. A localhost URL or proxy by itself does not protect
the underlying listener. Do not expose HTTP/GraphQL publicly (to serve GraphQL
reads to the public, see [Public read-only GraphQL](public-graphql.md)). The
`/state` endpoint has a pending crash fix, **I38**. This guide's status checks
avoid it; do not use state-inspection pages until that fix is available. The
libp2p port is separate and can be opened for peers. The dashboard below binds
only to loopback. For a remote server, use SSH local forwarding for both the
dashboard port and HTTP port, then open the dashboard on your own machine.

## Get the pinned source and build

Run these blocks in the same Bash shell, starting in an empty directory.
`SOURCE_REPO` can be a coordinator-provided review clone before publication; a
public operator must use the public repository and fetch the exact pin. If
fetching fails, stop: the maintainer must publish the commit, not substitute an
upstream release. The revision must include the checked-in Mesa mainnet genesis
(R40). For the m9 results alone, use `f2ce649c75f5e29a83def47cce5283bc3c774c82`,
which predates this reproducibility pack. Pin the integrated R40 commit when it
is published.

```bash
SOURCE_REPO=${SOURCE_REPO:-https://github.com/minanode/mina-rust.git}
PIN=${PIN:?set PIN to a published commit containing the R40 reproducibility pack}
git clone --no-checkout "$SOURCE_REPO" mina-rust
cd mina-rust
git fetch origin "$PIN"
git checkout --detach "$PIN"
export CARGO_TARGET_DIR="$PWD/target-operator"
export CARGO_BUILD_JOBS=8
HTTP=${HTTP:-3410}
P2P=${P2P:-8610}
DASHBOARD=${DASHBOARD:-4211}
export HTTP DASHBOARD
RUN="$PWD/operator-mainnet"
mkdir -p "$RUN"
# On the shared development machine, set LOCK to its central lock.sh first.
LOCK=${LOCK:-$PWD/tools/regression/lock.sh}
rustup toolchain install 1.92 --profile minimal --no-self-update
/usr/bin/time -p "$LOCK" build -- \
  cargo build --locked --release -p mina-cli --bin mina
MINA="$CARGO_TARGET_DIR/release/mina"
"$MINA" node status --help
```

The source and dependencies must come from Git/crates.io, not prebuilt files on
a maintainer's machine. `--locked` preserves the Cargo dependency revisions. The
first build can take substantial time; see the repository rehearsal report at
private implementation notes for what was actually measured and what remains
untested.

The isolated rehearsal built the native CLI in **6m52s** (**8m38s** including
the shared-machine build queue), but did not start mainnet because the
publication gates below were unresolved. The frontend built and served HTTP;
browser rendering was not verified. These are not end-to-end sync timings.

## Obtain the mainnet genesis

The native regression runs use `mesa-mainnet-without-ledgers.json`, a 6,222 byte
**Rust prebuilt genesis description**, not OCaml's small daemon config. It
includes ledger hashes, total currencies and the genesis winner's stake proof,
but no ledger accounts. The node downloads current ledgers from peers.

The exact file used by the milestone nodes is checked into `genesis_ledgers/`.
Verify its companion SHA256 before starting:

```bash
(cd genesis_ledgers && sha256sum --check mesa-mainnet-without-ledgers.json.sha256)
GENESIS="$PWD/genesis_ledgers/mesa-mainnet-without-ledgers.json"
```

The canonical file is exactly 6,222 bytes. Regeneration with current tooling
preserves ledger roots but adds runtime fork metadata; it does not reproduce
these legacy bytes. See
[Reproduce our results](../developers/reproduce-results.mdx) for the root check
and the unpublished inputs needed for replay and proving.

A non-producing node builds verification indexes from embedded data when its
cache is absent. The native binary embeds Mesa devnet prover circuits; mainnet
prover circuits still require a verified local dump. To opt out of embedded
devnet circuits, build `mina` with `--no-default-features` and explicitly enable
the P2P features you need. Prover circuits are not required for this
non-producing recipe. A warning that a verifier cache could not be downloaded
can be followed by local index construction; allow startup time and inspect the
subsequent log. Do not enable block production or SNARK work using this guide.
Verified mainnet prover blobs and release binaries remain follow-ups. When a
Mesa devnet binary is published, download its Linux tarball and check it against
the accompanying `SHA256SUMS` before unpacking. No download is available from
this guide yet.

## Start mainnet

Set ports to unused values. On the shared development machine use your assigned
slot `N`: HTTP `3400 + 10*N`, libp2p `8600 + 10*N`, dashboard `4210 + N`. The
following defaults use slot 1; change all three together if needed.

```bash
mkdir -p "$RUN/data"
"$LOCK" node -- "$MINA" node --network mainnet \
  --work-dir "$RUN/data" --port "$HTTP" --libp2p-port "$P2P" \
  --genesis-config "$GENESIS" \
  --peer-list-url https://storage.googleapis.com/seed-lists/mainnet_seeds.txt \
  --disable-filesystem-logging >"$RUN/node.log" 2>&1 &
NODE_PID=$!
printf '%s\n' "$NODE_PID" >"$RUN/node.pid"
"$MINA" node status --node "http://127.0.0.1:$HTTP"
```

The last command may initially fail while the node initializes. Do not start
another node: check the log and retry status. The lock wrapper holds a node slot
for the lifetime of the process. No producer key is needed. Let the shell stay
open; this recipe does not install a background service.

## Check progress and health

```bash
"$MINA" node status --node "http://127.0.0.1:$HTTP"
"$MINA" node status --node "http://127.0.0.1:$HTTP" --json
"$MINA" node status --node "http://127.0.0.1:$HTTP" --check
```

Expect network `mainnet`, a moving best-tip height, connected peers and
ultimately sync status `Synced`. The Mesa mainnet chain ID must be
`0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886`. `--check`
exits 0 only when synced, with at least one connected peer and a tip no older
than 1,800 seconds; it exits 1 for an unhealthy or unreachable node. Use
`--max-tip-age SECONDS` and `--min-peers COUNT` to change those thresholds. An
alert during initial sync is expected. `--status-only` requests only `/status`
when a restricted monitoring proxy does not allow optional build and
action-statistics endpoints. Counts shown for verification are recent action
statistics, not an all-time or all-block validation receipt. The P2P memory
field is not process RSS. Use `ps -o pid,rss,etime -p "$NODE_PID"` for RSS in
KiB.

### Liveness and readiness probes

The HTTP server has two probe endpoints, with the same split as OCaml's
`mina-healthcheck daemon-status` (liveness) and `mina-healthcheck ready`
(readiness):

- **`GET /healthz`** (liveness) answers 200 whenever the node answers, also
  while it bootstraps, catches up or has no peers. Use it for a liveness probe:
  it does not restart a node for being unsynced.
- **`GET /readyz`** (readiness) answers 200 only when the node reached `Synced`
  in the last 1,800 seconds and has at least one ready peer, otherwise 503 with
  a JSON `error` listing every failing condition, for example
  `not synced, peer count 0 < 1`. Right after a start, on the genesis block or
  on a tip restored from the frontier store, the node says `Connecting` (no peer
  yet) or `Listening` (no best tip from a peer yet) rather than `Synced`, then
  `Bootstrap` or `Catchup` while it syncs, as OCaml's `daemon status` does.

Bootstrap can take several minutes, more on a slow host, so do not use `/readyz`
or `node status --check` as a liveness probe. A Kubernetes example:

```yaml
startupProbe:
  httpGet: { path: /healthz, port: 3000 }
  periodSeconds: 10
  failureThreshold: 90
livenessProbe:
  httpGet: { path: /healthz, port: 3000 }
  periodSeconds: 30
  failureThreshold: 5
  timeoutSeconds: 10
readinessProbe:
  httpGet: { path: /readyz, port: 3000 }
  periodSeconds: 30
  timeoutSeconds: 10
```

Use the node's `--port`. The HTTP server listens on every interface, so expose
only the probe paths through a firewall or proxy.

## Build and open the dashboard

This can be built independently of the mainnet publication gate, but a live node
is required to see progress. Configure the native dashboard before building: the
default development configuration points at other nodes. Use Node.js 24 and a
fresh install from the checked-in npm lockfile.

```bash
cd frontend
CYPRESS_INSTALL_BINARY=0 npm ci
python3 - <<'PY'
import os
from pathlib import Path
port = int(os.environ['HTTP'])
Path('src/environments/environment.local.ts').write_text('''
import { MinaEnv } from '@shared/types/core/environment/mina-env.type';
export const environment: Readonly<MinaEnv> = {
  production: false,
  identifier: 'Mesa local node',
  canAddNodes: false,
  showWebNodeLandingPage: false,
  globalConfig: { features: { dashboard: [] } },
  configs: [{ name: 'Mesa native node', url: 'http://127.0.0.1:%d' }],
};
''' % port)
PY
/usr/bin/time -p "$LOCK" build -- npm run build:local
node node_modules/http-server/bin/http-server dist/frontend/browser \
  -a 127.0.0.1 -p "$DASHBOARD" -c-1 >"$RUN/dashboard.log" 2>&1 &
DASHBOARD_PID=$!
cd ..
```

Open `http://127.0.0.1:4211/` (use your chosen dashboard port). The dashboard
connects to your native node, not to a web node. Enter via `/` if a direct route
refresh gives a 404 on this simple static server. Only the dashboard is enabled
above; its typed `/state/peers` and `/state/message-progress` routes are
distinct from the generic `/state` inspection endpoint. Do not navigate to
generic state inspection. The status CLI is the acceptance check if the UI is
unavailable.

## Logs, stopping and restarting

```bash
tail -n 80 "$RUN/node.log"
du -sh "$RUN" "$CARGO_TARGET_DIR"
kill -TERM "$NODE_PID"
wait "$NODE_PID"
kill -TERM "$DASHBOARD_PID"
wait "$DASHBOARD_PID"
```

Stop only PIDs started by this shell; `wait` can return a nonzero exit status on
termination. Stop the dashboard only if you started it. Preserve logs you need
before rerunning: `>` overwrites the old log. Restart by repeating the same
start block with the same network, genesis, ports and data directory. The node
keeps a frontier store in its data directory by default: a restart restores from
it instead of bootstrapping when the store passes its checks and isn't too old,
and otherwise bootstraps, saying why. `--no-frontier-store` turns it off.
`--frontier-store-load-timeout` defaults to 60 seconds and accepts 1–300. Do not
mix devnet and mainnet data directories. Without an explicit key each start
generates a new peer identity. No secret key needs to be pasted into these
commands. Rotate logs for unattended use; service supervision and durable
restart behavior are outside this recipe.

## Devnet

Mesa devnet's `genesis_ledgers/mesa-devnet-without-ledgers.json` **is checked
in** and embedded as the default devnet genesis, from P1c's verified tarball
path. It needs no downloaded mainnet genesis. After building, use a separate run
directory, the same protected ports (after stopping mainnet), and start:

```bash
RUN="$PWD/operator-devnet"
mkdir -p "$RUN/data"
"$LOCK" node -- "$MINA" node --network devnet \
  --work-dir "$RUN/data" --port "$HTTP" --libp2p-port "$P2P" \
  --genesis-config "$PWD/genesis_ledgers/mesa-devnet-without-ledgers.json" \
  --peer-list-url https://storage.googleapis.com/seed-lists/devnet_seeds.txt \
  --disable-filesystem-logging >"$RUN/node.log" 2>&1 &
NODE_PID=$!
```

Use the same status and dashboard procedure. Expect network `devnet` and chain
ID `ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8`. This
section is not a mainnet sync result. Stop this process by its PID too.
