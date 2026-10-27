# @minanode/node-client

A typed client for a Mina Rust node's HTTP API and GraphQL. It covers the
requests, the parsing into plain types, the freshness rules, and the wording a
UI needs for sync, roles and errors. React hooks are in a separate entry point.
The node dashboard (`ui/dashboard`) reads the node only through this package.

It ships source (TypeScript), like the design system: the app's bundler compiles
it. The main entry has no runtime dependencies. React 19 is an optional peer,
needed only for `@minanode/node-client/react`.

## Install

In a workspace app under `ui/`:

```json
"dependencies": { "@minanode/node-client": "0.1.0" }
```

Then add the app to `workspaces` in `ui/package.json` and run `npm install`.
Vite and Next.js compile the `.ts` sources as they are.

## The client

```ts
import { createNodeClient, describeError, kindOf } from "@minanode/node-client";

const node = createNodeClient({ baseUrl: "http://127.0.0.1:3000" });

try {
  const status = await node.status();
  console.log(status.phase, status.bestTip?.height);
} catch (error) {
  console.log(kindOf(error), describeError(error));
}
```

`createNodeClient(options)` takes:

- `baseUrl`: where the node is. `""` (the default) is the page's own origin,
  which is what the dashboard the node serves uses. Any other origin works too,
  with or without a path. Trailing slashes are dropped. A node answers reads
  from any origin. It takes `POST /snarker/control` only from pages on its own
  machine, its own dashboard, and origins listed with `--worker-control-origin`,
  so an app elsewhere can show the worker but not change it unless the operator
  lists it.
- `fetch`: the fetch to use (tests, other runtimes). The default is the global
  one.
- `timeoutMs`: how long to wait for an answer before giving up (default 15 s).

| Method                                  | Request                                          | Returns                                   |
| --------------------------------------- | ------------------------------------------------ | ----------------------------------------- |
| `status()`                              | `GET /status`                                    | `NodeStatus`                              |
| `buildInfo()`                           | `GET /build_env`                                 | `BuildInfo`                               |
| `syncRestore()`                         | `GET /stats/sync?limit=1`                        | `RestoreSummary \| null`                  |
| `producerStats()`                       | `GET /stats/block_producer`                      | `ProducerStats \| null` (no producer)     |
| `workerControl()`                       | `GET /snarker/control`                           | `WorkerControl`                           |
| `changeWorker(change)`                  | `POST /snarker/control` with the control header  | `WorkerControl` after the change          |
| `includedWork()`                        | `GET /snarker/included`                          | `IncludedWork`, with the tip and market   |
| `report(options?)`                      | `POST /report` with the report header            | `Report`: title, markdown, issue link     |
| `crashNotice()`                         | `POST /report/crash` with the report header      | `CrashNotice \| null`                     |
| `transactionPool()`                     | `GET /transaction-pool`                          | `PendingTransaction[]`, highest fee first |
| `peers()`                               | `GET /state/peers`                               | `Peer[]`                                  |
| `raw(route)`                            | `GET /status` or `GET /build_env` (`RAW_ROUTES`) | the JSON as it comes                      |
| `bestChain(length?)`                    | GraphQL `bestChain` (default 20 blocks)          | `Block[]`, newest first                   |
| `block(id)`                             | GraphQL `block` by height or state hash          | `Block \| null`                           |
| `account(publicKey)`                    | GraphQL `account`                                | `Account \| null`                         |
| `getJson(path, parse)`                  | any GET                                          | what `parse` returns                      |
| `postJson(path, body, parse, headers?)` | any POST of JSON                                 | what `parse` returns                      |
| `graphql(query, variables, parse)`      | `POST /graphql`                                  | `parse(data)`; an error throws `refused`  |
| `url(path)`                             | none                                             | `path` on the node, for links             |

Every method takes an optional `AbortSignal` as its last argument.

### Errors

A failed request throws a `NodeError` with a `kind`, the HTTP `status` when
there was one, and the node's own message:

| `kind`    | When                                                                                       |
| --------- | ------------------------------------------------------------------------------------------ |
| `offline` | No answer: the node is down, restarting, unreachable, or the request timed out             |
| `absent`  | 404: the node doesn't serve the route (an older build, a public node, a feature off)       |
| `refused` | 400, 403, 405, 415, or a GraphQL error: refused with its reason (control off, a bad input) |
| `failed`  | Any other error status                                                                     |
| `invalid` | The answer isn't the shape the client parses                                               |

Helpers: `kindOf(error)` gives the kind of anything thrown (`failed` if it isn't
a `NodeError`), `describeError(error)` gives its message, and
`isUnreachable(error)` says whether the node isn't answering usefully (`offline`
or `failed`) rather than declining.

### The rest of the main entry

- **Status and sync** (`status`, `sync`): `NodeStatus`, `Peer`, `peerCounts`,
  `peerHost`, `BuildInfo`. `syncView(input, now)` turns a status into a
  `SyncView`: a key (`Starting`, `Connecting`, `Listening`, `Bootstrap`,
  `Catchup`, `Synced`, `NotSynced`, `NoAnswer`), a label, a tone for a `Signal`,
  a headline and a detail. Its constants are `STATUS_POLL_MS`,
  `STATUS_STALE_MS`, `TIP_QUIET_MS`, `SYNCED_TIP_MAX_AGE_MS`,
  `SYNCED_MAX_BEHIND_PEERS` and `K`. `peerNote` describes how far ahead the
  peers are.
- **Restore** (`restore`): `RestoreSummary` and `restoreHeadline`.
- **SNARK worker** (`worker`): `WorkerControl`, `workerView` (label, tone and
  detail), `startChange`, `STOP_CHANGE`, `feeInput` and `PUBLIC_KEY_PATTERN` for
  the start form, `describeJob`, and `IncludedWork`. `WorkerControl.strategy`
  (`SnarkerStrategy`, `Random` or `Sequential`, from `--snarker-strategy`) is
  null from nodes that don't send it; `strategyLabel` puts it in words. Its
  `bestTipHeight` and `market` are null from nodes that don't send them.
  `CONFIRMATIONS` (15, Mina's recommended wait for a payment) with
  `confirmations(entry, tip)`, `confirmation(entry, tip)`,
  `confirmationTotals(work, tip?)`, `confirmationLabel` and `confirmationTone`
  say whether bought work is confirmed, pending, dropped or unknown.
  `SnarkMarket` (the best chain's newest purchases and the pool's offers per
  prover: the node lists the ten cheapest others and its own keys, with
  `otherProvers` and `highestOtherFee` for all),
  `marketProvers(market, ownKeys)` (the others, their count and fee range), and
  `feeComparison(market, key, fee, ownKeys?)`, a sentence on how this worker's
  fee compares.
- **Bug report** (`report`): `Report` (title, fingerprint, the markdown, the
  pre-filled issue link and whether the whole report fits in it, a file name)
  with `reportDocument(report)` for a download, `CrashNotice`, and
  `REPORT_HEADER`. The node answers these only for its operator, like worker
  control: this machine, its own dashboard, or a `--worker-control-origin`.
- **Block producer** (`producer`): `ProducerStats`, `WonSlot`, `Attempt` (with
  its stage `times`), `VrfEpoch` (each epoch's search for won slots; the stake
  fields and `wonSlots` are null from builds that don't send them),
  `attemptView`, and `upcomingWonSlots(stats, startedMs?)`: the won slots ahead,
  soonest first, with the one whose attempt the node has scheduled (it leaves
  `futureWonSlots` then). Given the latest run's start, it leaves out an attempt
  the run before a restart scheduled.
- **What became of each won slot** (`production`):
  `attemptReports(stats, uptimeSecs)` gives each attempt an `AttemptReport`: its
  `outcome` (upcoming, producing, previousRun, broadcast, canonical, orphaned,
  lostTie or missed), whether it has `ended`, a label, tone and the reason in
  plain words, its `stages` (won, built, proof start and end, injected, applied,
  broadcast) with each one's offset from the slot's start, the proof's time and
  when the block went out. An attempt the run before the latest restart left
  unfinished is `previousRun` ("From before the restart") while its slot is not
  over, and gives way once this run schedules the slot again; once its slot is
  over, it is missed, `interrupted`. A canonical block is labelled Confirmed
  from `CONFIRMATIONS` blocks on top and Pending below, as included SNARK work
  is. `productionSummary(reports, ahead)` counts the outcomes and gives the
  median and worst proof and broadcast times and the coinbase of the canonical
  blocks. `epochExpectations(stats.vrfEpochs)` sets the wins to expect, slots
  checked times `winProbability(stake, total)` (1 − (1/4)^share, Mina's f =
  3/4), and `binomialRange(n, p)`, the exact central 95% of the wins by chance
  alone (honest for few slots or few wins), against those found; `reportsStake`
  says whether the node sends the stake at all. `slotDurationMs(stats)` reads
  the slot length from the node's slot times (`SLOT_MS`, 90 s, otherwise).
- **Chain** (`chain`): the GraphQL queries (`RECENT_BLOCKS`, `BLOCK_BY_HASH`,
  `BLOCK_BY_HEIGHT`, `ACCOUNT`) with their parsers, `Block`, `Account`,
  `blockQuery(id)` (null for an id that is neither a height nor a state hash),
  `commandFees`, `blockCounts`, `isTimed`, `decodeMemo`.
- **Mempool** (`mempool`): `PendingTransaction`.
- **Formatting** (`format`): `formatMina`, `minaToNanomina`, `formatHeight`
  (`#578883`), `formatCount`, `formatAge`, `formatDuration`, `formatOffset`
  (`+3.9 s`, `44 min before`), `utcTime`, `utcDateTime`, `shortHash` and
  `plural`.
- **Links** (`explorer`): `explorerBlockUrl` and `explorerTransactionUrl` for
  the public explorer of the node's network.
- **JSON** (`json`): small readers (`obj`, `arr`, `str`, `num`, `bool`,
  `nanosToMs`) for writing your own parser to pass to `getJson`.

## React

```tsx
import { createNodeClient } from "@minanode/node-client";
import {
  NodeClientProvider,
  NodeStatusProvider,
  useNodeStatus,
  usePeers,
} from "@minanode/node-client/react";

const client = createNodeClient({ baseUrl: "http://127.0.0.1:3000" });

root.render(
  <NodeClientProvider client={client}>
    <NodeStatusProvider>
      <App />
    </NodeStatusProvider>
  </NodeClientProvider>,
);

function PeerCount() {
  const peers = usePeers();
  if (peers.loading) return <p>Loading…</p>;
  if (peers.error) return <p>No answer</p>;
  return <p>{peers.data?.filter(p => p.connected).length} connected</p>;
}
```

Every hook returns a `Polled<T>`:
`{ data, error, updatedAt, loading, refresh }`. `data` keeps the last good
answer when a later request fails, so a page can show it with its age (see
`DataAge` in the design system). `loading` is true only before the first answer.

Polling asks again every `intervalMs` while the tab is visible. It stops while
the tab is hidden and asks at once when the tab comes back. A change of key
(another block, another account, another `baseUrl`) starts over. `paused` keeps
the last answer and asks again only on `refresh()`.

| Hook                                    | Polls                                                                 | Default interval |
| --------------------------------------- | --------------------------------------------------------------------- | ---------------- |
| `useNodeStatus()`                       | shared `status`, `build` and `peers` counts from `NodeStatusProvider` | 5 s; build 60 s  |
| `useSyncView(now)`                      | the sync phase in words, from the shared status                       | none             |
| `useSyncRestore(ms?)`                   | `syncRestore()`                                                       | 15 s             |
| `useWorkerControl(ms?)`                 | `workerControl()`                                                     | 5 s              |
| `useIncludedWork(ms?)`                  | `includedWork()`                                                      | 15 s             |
| `useCrashNotice(ms?)`                   | `crashNotice()`                                                       | 60 s             |
| `useProducerStats(ms?)`                 | `producerStats()`                                                     | 10 s             |
| `useTransactionPool(ms?)`               | `transactionPool()`                                                   | 5 s              |
| `usePeers(ms?)`                         | `peers()`                                                             | 5 s              |
| `useBestChain(length?, ms?)`            | `bestChain(length)`                                                   | 10 s             |
| `useBlock(id \| null)`                  | `block(id)`; null asks nothing                                        | 60 s             |
| `useAccount(publicKey \| null, ms?)`    | `account(publicKey)`; null asks nothing                               | 30 s             |
| `useRaw(route, { paused, intervalMs })` | `raw(route)`                                                          | 5 s              |
| `usePoll(key, fetcher, ms, { paused })` | anything; a null key asks nothing                                     | yours            |

`useNodeClient()` gives the client from `NodeClientProvider`, or a client for
the page's own origin when there's no provider. Use it for actions, such as
`client.changeWorker(change)`.

## Tests

`npm test` (Vitest, Node environment) runs the parsers against fixtures recorded
from devnet nodes (`src/fixtures/`), the sync rules, the formatting, and the
client against a stub fetch: paths, base URL, headers, error kinds and GraphQL.
