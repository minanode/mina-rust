---
title: SNARK worker
sidebar_position: 3
description:
  Run the node's SNARK worker from the command line or the dashboard, see the
  blocks that bought its work, and turn it off for good.
---

# SNARK worker

A node can prove SNARK work for the network and sell it. Block producers buy the
cheapest work for each job from their snark pool and pay its fee to the worker's
public key. The worker needs only that public key: no private key is read or
stored.

## Start it from the command line

```bash
mina node --network devnet --run-snarker B62q... --snarker-fee 0.004
```

`--snarker-fee` is in MINA (default 0.1). The flags set the worker at every
start.

## Start it from the dashboard

Start the node with `--allow-worker-control` and open the node's own dashboard
at `http://<node address>:<HTTP port>/snark-worker` (the node serves it on its
HTTP port unless started with `--no-dashboard`; the developer frontend's SNARKs
→ Worker page works too). Enter the public key and the fee, and start the
worker. While it runs you can change the key or the fee, or stop it. Either way
the worker restarts or stops: a proof it was making is dropped and its job
released, so the worker can take it again at its new setting.

The HTTP port listens on `127.0.0.1` by default. On a server, use
`ssh -N -L 3000:127.0.0.1:3000 you@server` and open
`http://127.0.0.1:3000/snark-worker` locally. For a private network, set
`--http-addr` to its private IP address; `--http-addr 0.0.0.0` listens on all
IPv4 interfaces. Keep that port private when using `--allow-worker-control`.

<!-- prettier-ignore-start -->

:::caution Keep the HTTP port private

`--allow-worker-control` lets anyone who can reach the node's HTTP port choose
where the fees go. Use it only on a node whose HTTP port only you can reach. The
node refuses it together with `--public-graphql`, and the public API never
serves the worker routes.

:::

A web page can reach the HTTP port through your browser, so the node takes a
change only from pages of this machine (`localhost`, `127.0.0.1` or `[::1]`, any
port), from its own dashboard opened at an IP address (for example
`http://192.0.2.44:3000` over Tailscale or a LAN; only while the node serves the
dashboard, so not with `--no-dashboard`), and from origins you list with
`--worker-control-origin` (repeatable, or `MINA_WORKER_CONTROL_ORIGINS` as a
comma-separated list), for a dashboard served from elsewhere or opened under a
host name: for example `--worker-control-origin https://dash.example.org`. A
change from any other page is refused. A host name is never trusted by itself:
another site can make its own name resolve to your node (DNS rebinding).

The node serves no page that loads scripts from elsewhere, since such a script
would run on the node's own address and could change the worker: the default
build has no GraphiQL and no API docs UI (Scalar, Stoplight or Swagger UI). A
developer build that adds GraphiQL, Scalar or Stoplight doesn't serve them while
worker control is on, and says so in its start-up log. For GraphQL, the
dashboard's GraphQL page shows the endpoint, `/graphql`, to use with any GraphQL
client or curl. Reading the worker's state (`GET /snarker/control`) is open to
every page, as `/status` is, so a dashboard served from elsewhere shows a node
whose control is off as off.

<!-- prettier-ignore-stop -->

The node keeps the setting you choose in `<work dir>/snark-worker.json` (the
work dir is `~/.mina` unless `-d` or `MINA_HOME` says otherwise), so a restart
brings the worker back with the same key and fee, with or without
`--allow-worker-control`. `--run-snarker` wins over the saved setting.

## Turn it off for good

- Turn the worker off in the dashboard: that deletes `snark-worker.json`.
- Or stop the node and delete `<work dir>/snark-worker.json`.
- And start the node without `--run-snarker`.

## Circuit blobs

The native `mina` binary includes the Mesa devnet prover circuits. A devnet
worker needs no separate circuit download or host. For mainnet, or a binary
built without the `embedded-circuits` feature, point
`MINA_CIRCUIT_BLOBS_BASE_DIR` at a directory containing the network's circuit
files (`tools/mesa-circuits/generate.sh` can make them). The loader also checks
the home and system circuit directories, then GitHub releases. A missing mainnet
circuit stops the worker with an error shown on the dashboard.

To opt out of embedded-first, build `mina` without `embedded-circuits`: use
`--no-default-features` and explicitly enable the P2P features you need. That
build uses the directory and GitHub lookup.

## Capture jobs for a bug report

The worker checks every proof it makes before offering it. A job it cannot prove
is dropped: the node logs `SnarkWorkerWorkFailed` with the error and offers
nothing for it. To keep such a job for a bug report, start the native node with
`MINA_SNARK_WORKER_CAPTURE_DIR` set to a writable directory:

```bash
MINA_SNARK_WORKER_CAPTURE_DIR=$HOME/snark-captures \
  mina node --network devnet --run-snarker B62q... --snarker-fee 0.004
```

For each failed job the node writes `<time>-<pid>.bin`, the whole work spec and
the SOK message (binprot), and `<time>-<pid>.error.txt` with the error, and logs
`SnarkWorkerFailureCaptured` with the path. With
`MINA_SNARK_WORKER_CAPTURE_ZKAPP=1` as well (only `1` turns it on; `0` leaves it
off), it also saves every zkApp command job that proved, as
`<time>-<pid>.proved.bin`, to audit which zkApps the worker proves. Both are off
by default, and the web node has neither. The node creates the directory, and
any parent it needs, readable by its owner only (mode 0700), and every file in
it too (0600): a work spec holds the ledger accounts its transactions touch.

Nothing deletes captures, so watch the directory's size. Each file is one work
bundle: 18 to 22 KB for the devnet zkApp bundles of the run that introduced the
feature (38 files, 0.8 MB in six hours); a zkApp command with many account
updates makes a larger one. Failure captures alone stay small, one file per
failed job; zkApp capture grows with the network's zkApp traffic, so turn it off
when the audit is done.

Developers replay a devnet capture without a node, with the Mesa devnet circuit
blobs (see [Circuit blobs](#circuit-blobs)):

```bash
MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs \
  MINA_ZK1_CAPTURE=$HOME/snark-captures/<file>.bin \
  cargo test --release -p mina-tree --lib reprove_captured_zkapp_failure -- --ignored --nocapture
```

## The fee rule

If the key's account doesn't exist on chain yet, nodes (OCaml's and ours) only
accept work whose fee is at least the account creation fee, 1 MINA. The
dashboard refuses a lower fee for such a key and says so: fund the account
first, or ask at least 1 MINA. A key whose account can't receive (its access or
receive permission isn't None) can't sell work at all.

## Job selection

The worker takes only a job that no worker has committed to and that has no work
in the pool as cheap as the worker's fee. `--snarker-strategy` decides which of
those jobs it takes next:

- **`rand`** (the default): any of them, at random. OCaml's daemon does the same
  by default (`--work-selection rand`), so workers spread over the scan state
  rather than racing for the same job.
- **`seq`**: the first in scan-state order, the oldest work, as OCaml's
  `--work-selection seq`.

```bash
mina node --network devnet --run-snarker B62q... --snarker-fee 0.004 \
  --snarker-strategy seq
```

The flag applies to every worker the node runs: one started with
`--run-snarker`, from the dashboard, or from the setting the dashboard saved.
For each job it takes, the node logs a `SnarkWorkerJobTaken` line with the job's
scan-state position (`order`), the lowest position it could have taken
(`lowest_order`) and how many jobs it could have taken (`eligible`). OCaml's
third method, `roffset`, isn't implemented. The OCaml reference is the
[`--work-selection` flag](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L946-L953)
and its
[selection methods](https://github.com/MinaProtocol/mina/tree/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector)
(4.0.2).

## Block production takes precedence on a native node

When the same native node produces blocks and runs a SNARK worker, it stops
starting new worker jobs 420 seconds before a known won slot and while it is
producing a block. The 420 second default matches the per-proof work
reassignment window. It is deliberately bounded: a bundle of many zkApp proofs
can run longer, and a much longer lead could leave the worker idle through
several slots. Set a different lead in seconds with
`--snarker-block-first-lead 600` (up to 4200 seconds), or use
`--snarker-block-first-lead 0` to turn this scheduling rule off. The flag also
governs a worker started from the dashboard or restored from
`snark-worker.json`. Without a producer key, the worker runs normally regardless
of the flag.

The dashboard says **Paused: a block of ours is due** while the worker waits to
take another job. It resumes after the slot. A job already proving keeps
running: the native in-process prover cannot stop its computation on cancel, so
cancelling it would discard useful work without freeing the CPU for the block
proof. A won slot learned late can therefore still overlap a worker proof; the
lead is a best-effort guard, not a proof deadline guarantee.

## Work included in blocks

The node logs every best-chain block that bought work from the worker's keys
(the last eight keys it has used): its height, time, producer, the bundles and
their fees, and whether the block is pending, final (at or behind the root) or
dropped by a reorg. It keeps the last 500 blocks, and totals since the log
began, in `<work dir>/snark-worker-included.json`.

The dashboard's SNARK worker page shows the log with links to the block
explorer, and counts each block's confirmations: the blocks on top of it. Work
is **confirmed** once its block has 15 confirmations, Mina's recommended wait
for a payment ("15 blocks which provides a 99.9% confidence that the transaction
will not be reversed",
[Lifecycle of a Payment](https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment)),
and **pending** until then; **dropped** means another block took its place. The
node's own finality, 290 blocks behind the tip, comes later and changes nothing
on the page.

Blocks buy work only when the chain needs proofs, often many jobs at once and
then none for hours, so a new worker can show nothing bought for a while. The
page's market section shows what the node's best chain bought from every prover
(the newest purchases, their fees and provers) and the proofs in its SNARK pool
per prover, with how this worker's fee compares. A job (OCaml's work bundle) is
one or two proofs that a block buys together, for one fee.

## HTTP API

These routes are on the node's private HTTP API only.

| Route                   | What it does                                                      |
| ----------------------- | ----------------------------------------------------------------- |
| `GET /snarker/control`  | The worker: allowed, on or off, key, fee, state, job, latest jobs |
| `POST /snarker/control` | Start, re-price or stop it (needs `--allow-worker-control`)       |
| `GET /snarker/included` | Included work and its totals, the best tip, and the market        |

```bash
curl -s -X POST http://127.0.0.1:3000/snarker/control \
  -H 'Content-Type: application/json' -H 'X-Mina-Worker-Control: 1' \
  -d '{"enabled": true, "public_key": "B62q...", "fee": "0.004"}'
curl -s -X POST http://127.0.0.1:3000/snarker/control \
  -H 'Content-Type: application/json' -H 'X-Mina-Worker-Control: 1' \
  -d '{"enabled": false}'
```

A change needs `Content-Type: application/json` and the `X-Mina-Worker-Control`
header; a browser can send neither from a page without asking the node first,
and the node says yes only to the origins above.

Fees in responses are in nanomina. `GET /snarker/included` also has
`best_tip_height` (a block's confirmations are this minus its height) and
`market`: `blocks` and `buying_blocks` (the newest 290 best-chain blocks at
most, and those of them that bought work), `bundles`, `purchases` (the newest
five buying blocks with their height, time, bundles, provers and lowest and
highest fee), `offers` (completed work in the pool per prover: bundles, lowest
and highest fee; the ten cheapest other provers and the worker's own keys),
`other_provers` (how many provers besides the worker's keys have work in the
pool) and `highest_other_fee`. The node builds it in time linear in the pool and
the blocks' works, and the answer stays small however large the pool. Nodes
before this release send neither. A bad key or fee gets HTTP 400 with the
reason; control without the flag, without the header or from another page's
origin gets HTTP 403.
