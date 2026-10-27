---
title: Public read-only GraphQL
sidebar_position: 2
description:
  The --public-graphql flag, which lets a node answer a read-only GraphQL subset
  to anyone who can reach its HTTP port.
---

# Public read-only GraphQL

By default a node's GraphQL endpoint (`POST /graphql` on the HTTP port) answers
every query the schema has, including peer addresses, SNARK pool contents and
local telemetry, and the HTTP port serves the node dashboard. That is meant for
the node's operator, which is why the quick start says to keep the HTTP port
private.

With `--public-graphql` (or `MINA_PUBLIC_GRAPHQL=true`) the node answers only a
read-only GraphQL subset, a minimal cached status and health probes. All other
HTTP routes are absent on this listener, even with `--enable-tx-submission`:

```bash
mina node --public-graphql --http-addr 0.0.0.0 ...
```

The HTTP listener otherwise binds to `127.0.0.1`, including in public GraphQL
mode. Use `--http-addr` with a specific interface address instead if preferred.

## What it answers

- **one query per request**: no mutations, subscriptions or batches, and one
  operation per document;
- **chain and ledger reads**: `syncStatus`, `networkID`, `genesisConstants`,
  `account`, `bestChain`, `block`, `genesisBlock`, `transactionStatus`,
  `pooledUserCommands` and `pooledZkappCommands`, with their blocks,
  transactions, zkApp commands and accounts;
- **a `daemonStatus` subset**: `stateHash`, `blockchainLength`, `syncStatus`,
  `chainId` and `consensusConfiguration`;
- **verification receipts**: `verificationReceipts`, what this node checked
  about up to 20 blocks (see below);
- **bounded lists**: `bestChain` needs `maxLength` from 1 to 20; the pool
  queries need `publicKey` or a `hashes`/`ids` filter. Every supplied `hashes`
  and `ids` list is limited to 10 entries, even with `publicKey`, and
  `verificationReceipts` to 20 `stateHashes`. Literal lists, variables and
  variable defaults obey the same limits.

## Verification receipts

The node keeps a receipt for each of the last 1,024 blocks it received: whether
it verified the block proof, checked the delta chain proof and the body against
the header, and applied the block (which includes checking the resulting staged
ledger hash against the header). A public client reads them by state hash:

```graphql
query VerificationReceipts($stateHashes: [String!]!) {
  verificationReceipts(stateHashes: $stateHashes) {
    stateHash
    height
    network
    arrival
    applied
    blockProof {
      status
      elapsedMs
    }
    deltaChain {
      status
      elapsedMs
    }
    bodyReference {
      status
      elapsedMs
    }
    verifierKey
  }
}
```

- **bounded**: at most one `verificationReceipts` selection and 20 state hash
  positions per public request (a page of recent blocks). Aliases, repeated and
  nested fragments, and skipped fields all count as selections. Exceeding either
  bound is refused with `PUBLIC_GRAPHQL_BAD_ARGUMENT` before any resolver or
  lookup runs. Thus one request makes at most one lookup in the node's receipt
  map, costing 30 towards the 1000 ceiling. An empty list makes no lookup.
  Without `--public-graphql`, the resolver caps each list at 20.
- **fixed fields**: `PublicVerificationReceipt` and `PublicVerificationCheck`
  are their own types, named field by field in the allow-list. They carry no
  receive time, absolute check time, failure list, counters or other operator
  telemetry, and they do not grow when the operator's receipt does. Check
  timestamps are omitted because initial header checks can coincide exactly with
  this node's receive time; private receipts retain those timestamps.
- **answers**: receipts come in the order asked, each block once. A block the
  node keeps no receipt for (never seen by this process, or older than its last
  1,024 blocks) is left out. `status` is `notRun`, `pending`, `passed`, `failed`
  or `skipped`; `elapsedMs` is a check duration when measured; `verifierKey` is
  the SHA-256 of the blockchain verifier index the proof was checked with.
  `arrival` is an ingestion/origin enum (`gossip`, `catchup`, `bootstrap`,
  `bestTip`, `local`), not an absolute timestamp.

These are local, observational receipts: what this process checked, not
consensus data and not a claim about other nodes. The operator's
`/stats/verification` route, with counters, failures and paging, stays private,
as does the `verification` GraphQL field.

The grant frontend's block and single-account views, the o1js daemon queries and
nine of the eleven Auro read documents in our fixture fit. Auro's
`tokenQueryBody` (`accounts`) and `tokenInfo` (`tokenOwner`) need a private
endpoint. `accounts(publicKey)` scans the ledger; a private `tokenOwner` index
miss builds a complete index for that ledger hash, including accounts whose
derived token has no holders. Later misses reuse that cached index. Both fields
are excluded from public mode, including default-token `tokenOwner`, rather than
assigned a misleading fixed cost.

## Public HTTP surface

Public mode registers exactly these routes on the existing HTTP listener:

- `POST /graphql`: the existing query allow-list and admission limits below;
- `GET /healthz` and `GET /readyz` (also HEAD): the existing health/readiness
  probes;
- `GET /status` (also HEAD): a separate, fixed-size public projection.

The public status contains only `chain_id` (a 64-character hex chain digest or
null) and `transition_frontier`, with `best_tip` (hash, height, block timestamp
in nanoseconds, or null) and `sync` (phase and event time in nanoseconds or
null). The phase is one of `Bootstrap`, `Catchup`, `Synced`, `Pending`
(`Pending` also while the node connects to peers or waits for their best tip,
the operator status's `Connecting` and `Listening`). It contains no peers, peer
IDs, IPs, addresses, keys, logs, build metadata, producer attempts, ledger
queues, pool contents, resource measurements or raw diagnostic strings.

All clients, HEAD requests and query strings share one five-second status cache.
Only one refresh runs at a time. A refresh still computes the private status
internally, then copies the named public fields; its work can scale with node
state. Cache hits clone only the fixed-size projection. Failed refreshes are
cached too, returning HTTP 503 with `{"error":"status unavailable"}`; a refresh
waits at most two seconds for RPC. This bounds traversal frequency, not the cost
of one refresh. The private response is never cached for public clients.

Every other route returns 404, including the formerly retained `/build_env`,
`/transaction-pool`, `/best-chain-user-commands`, `/stats/sync`,
`/stats/verification` and `/discovery/bootstrap_stats`. Adding routes to the
operator router cannot expand this set. Without `--public-graphql`, all operator
routes and their payloads, uncached status and the submission flag retain their
behavior. WebRTC signaling is private: use a separate relay for a browser node.

## Public dashboard preset

Build the native public observer with:

```bash
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  sh -c 'cd frontend && npx ng build --configuration public'
```

Serve `frontend/dist/frontend/browser` with SPA fallback and proxy `/api` to
this public node. The preset uses a same-origin `/api` base URL, fixed view-only
controls, and no saved custom nodes. The existing `native` and `webnode` R26g
presets only disable controls; they still use private operator APIs. Use
`public` for `--public-graphql`. Developer, native and browser-node presets are
unchanged.

Visible screens:

- **Recent blocks** (the landing page): the existing `RecentBlocks` GraphQL
  query with `bestChain(maxLength: 20)`; refreshes every ten seconds.
- **Block details**: the existing `BlockDetails` GraphQL query, including that
  block's commands. It does not fetch whole-chain REST commands.
- **Verification receipts**: a Verification column on Recent blocks and a
  receipt panel on Block details, read with `verificationReceipts` (one request
  for the 20 listed blocks, refreshed every ten seconds).
- **Account lookup**: the existing `LedgerAccount` query for one public key,
  using `account`, not the excluded full-ledger `accounts` field.
- **Header and freshness strip**: the public `/status` projection, polled every
  ten seconds. Missing peer/pool/producer measurements are unknown, not zero.

Hidden screens and panels (direct links redirect to Recent blocks):

- The operator **Dashboard**, including peers, sync history, RPC/message
  progress, and its verification panel (`/state/peers`, `/stats/sync`,
  `/state/message-progress`, `/stats/verification`).
- **Nodes**, **Network** (including bootstrap/discovery), **State**, **SNARKs**,
  **Resources**, **Fuzzing**, and browser-node startup. Their operator data has
  no public GraphQL equivalent.
- **Mempool**: its unfiltered whole-pool `/transaction-pool` and whole-chain
  `/best-chain-user-commands` reads do not fit the public policy's required pool
  filters and bounded best-chain query. Block commands remain available on the
  block screens.
- **Block Production**, **Benchmarks**, sending and key-upload controls.
- The local verification counters panel above Recent blocks (verified and failed
  counts, median proof time, recent failures; the private `/stats/verification`
  and `verification` field), peer comparison and peer/pool header pills, and the
  node build stamp (`/build_env`). The frontend's own build stamp remains.

The R29 Caddy draft's nine GET paths predate this boundary. For this preset,
reduce its `@api_read` paths to `/api/status /api/healthz /api/readyz`; keep
`POST /api/graphql`, strip `/api`, and return 404 for other `/api/*` paths. Keep
browser signaling on its separate relay if that deployment needs it. No live
Caddy or shared service configuration is changed by this task.

## What it refuses

- `accounts` and `tokenOwner`, which can require full-ledger scans;
- the send mutations (`sendPayment`, `sendDelegation`, `sendZkapp`), whatever
  `--enable-tx-submission` says;
- introspection (`__schema`, `__type`); `__typename` is allowed;
- peers and addresses (`getPeers`, `initialPeers`, `daemonStatus.peers`,
  `daemonStatus.addrsAndPorts`), the node's version, SNARK pool and worker
  (`snarkPool`, `pendingSnarkWork`, `currentSnarkWorker`), `trackedAccounts`,
  `verification` (the operator's receipt counters and pages), an account's
  `delegators` and a block's `protocolStateProof`;
- queries deeper than 12 levels, with more than 300 selections (fragments
  expanded) or 10 root fields, or over a cost of 1000. Account lookups, blocks,
  pool lookups and proofs count towards the cost, multiplied by the lists they
  are in, including non-null and nested list wrappers: 20 blocks with their
  coinbase receivers cost 600, 20 blocks with every command's sender account are
  refused.

Every other HTTP route returns 404 without an RPC, including `/state` and all
`/state/*`, `/accounts`, `/send-payment`, `/make_heartbeat`, operator
statistics, SNARK pool/worker routes, scan state, discovery routing table,
GraphiQL and all API documentation UIs. Unsupported methods on admitted routes
return 405; CORS preflight does not execute a handler.

A GraphQL policy refusal gets HTTP 400 before any resolver or RPC runs:

```json
{
  "errors": [
    {
      "message": "This public endpoint is read-only: only queries are allowed",
      "extensions": { "code": "PUBLIC_GRAPHQL_NOT_ALLOWED" }
    }
  ]
}
```

The codes are `PUBLIC_GRAPHQL_NOT_ALLOWED` (a field or operation outside the
subset), `PUBLIC_GRAPHQL_BAD_ARGUMENT` (a missing or out-of-range list bound)
and `PUBLIC_GRAPHQL_TOO_COMPLEX` (the size limits). GraphiQL is not served.

## What it does not do

- It does not limit the request rate: a bounded query can still be sent many
  times. Put the node behind a proxy that limits the rate, the body size (64 KiB
  is plenty). A GraphQL-only proxy should forward only `POST /graphql`; a
  dashboard proxy can additionally forward the listed GET routes. The node
  itself enforces the route boundary, even when its port is directly reachable.
- Its cost weights are admission estimates, not measured CPU or response-size
  guarantees. Full-ledger GraphQL scans and broad dashboard REST reads are
  excluded; bounded public requests still need rate limits.
- It is a daemon, not an archive: history, events and actions are not there,
  only the transition frontier and the current ledger.
