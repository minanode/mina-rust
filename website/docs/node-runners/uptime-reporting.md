---
title: Uptime, node status and node error reporting
sidebar_position: 3
description:
  The opt-in --uptime-url, --node-status-url and --node-error-url reporters,
  compatible with OCaml Mina 4.0.4's services, and how to see what they sent.
---

# Uptime, node status and node error reporting

`mina node` can send the three reports an OCaml Mina 4.0.4 daemon sends:

- **Uptime submissions** (`--uptime-url`): signed blocks and selected SNARK
  work, for the uptime service.
- **Node status** (`--node-status-url`): an unsigned summary of the node.
- **Node errors** (`--node-error-url`): an unsigned report when the node meets
  an error.

All three are **off by default**: a reporter runs only when you give its URL,
and each URL is independent. They are separate from the Prometheus metrics and
from `POST /make_heartbeat`, which keeps its own format.

The reports follow OCaml's formats, signatures and timing, and were checked
against requests captured from the OCaml daemon. Sending reports does not
register a key with any service: a receiving service decides which submitters it
accepts and how it scores them. Check with the service's operators before
pointing a production node at it.

## Uptime submissions

```sh
UPTIME_PRIVKEY_PASS='…' mina node --network mainnet \
  --uptime-url https://uptime.example.org/v1/submit \
  --uptime-submitter-key /keys/uptime-key
```

- `--uptime-url URL`: where to POST submissions. HTTP or HTTPS; the URL is used
  as given, no path is added.
- `--uptime-submitter-key FILE`: the encrypted secret key file of the account
  that signs submissions. It need not be the block producer's key. The file must
  be readable by its owner only (`chmod 600`).
- `--uptime-submitter-pubkey PUBLICKEY`: instead of a key file, sign with the
  key tracked for this public key in `<work dir>/wallets/store`, as OCaml looks
  it up in `<config dir>/wallets/store`: the file a `NAME.pub` file names when
  its first line is the key, else the file named after the key.
- `--uptime-send-node-commit-sha`: include the first 8 characters of this
  build's commit. Off by default.
- `--snarker-fee MINA` (alias `--snark-worker-fee`): the fee used with the
  submitter's public key in uptime work. Defaults to `0.1` MINA and can be set
  without enabling the general SNARK worker. Runtime worker fee changes apply to
  subsequent uptime attempts; stopping that worker retains its last fee.

The key's password is read from `UPTIME_PRIVKEY_PASS`, as OCaml reads it. It
must be set, even to an empty password: the node does not prompt, and it never
uses `MINA_PRIVKEY_PASS` for this key.

Exactly one of the two key options is needed with `--uptime-url`, and neither is
accepted without it. Errors name the option, never the file, the key or its
password.

Each option may also be set with an environment variable: `MINA_UPTIME_URL`,
`MINA_UPTIME_SUBMITTER_KEY`, `MINA_UPTIME_SUBMITTER_PUBKEY`,
`MINA_UPTIME_SEND_NODE_COMMIT_SHA` and `MINA_UPTIME_DRY_RUN_DIR`. The command
line wins over the variable. OCaml has no such variables; its option names work
unchanged.

### Dry run

`--uptime-dry-run-dir DIR` sends nothing. Each request is signed as usual and
its body, the exact bytes that would be POSTed to `--uptime-url`, is written to
`DIR/uptime-<created_at>-<best-tip|produced>-<state hash>.json`. Use it to
compare the node's submissions with OCaml's, for example with `cmp` or `jq`,
before pointing the node at a real service. The directory is created if missing.
OCaml has no such option.

The reporter skips startup, frontier restore and bootstrap until a real,
non-genesis frontier is available. It never saves or submits the temporary
genesis tip. Catch-up from an existing frontier can still report its current
tip, as OCaml does; a saved file alone does not establish that the node has
caught up to the network. Check `mina node status --check` and the reported
height and state hash in `/reporting/status` when assessing readiness.

What is sent, and when:

- The first submission is at the next slot boundary, then one every five slots.
  The interval follows the network's slot time: with 90-second slots it is 7.5
  minutes. Each sends the node's best tip as a whole block, with its proofs, in
  OCaml's binary form.
- If the node produces a block within four slots of a submission, it also waits
  for that block (up to three minutes after its slot starts) and sends it.
- Each submission is signed once and retried with the same bytes: up to eight
  attempts, four seconds apart. Only HTTP 200 succeeds. 400, 401, 411 and 413
  stop at once; other statuses (429 included) and connection errors are retried.
  Each attempt gives up after 60 seconds (10 seconds to connect).
- The next submission interrupts one still proving or being retried. A late
  proof is discarded and cannot be sent by a later submission.
- Mainnet submissions are signed for mainnet, devnet ones for testnets. A
  signature for one is invalid on the other.
- A submission is skipped, and logged, while the node isn't yet connected to the
  network or has no best tip.
- The best tip's submission chooses its SNARK work as OCaml's does. A block with
  no transactions goes alone, and so does one whose scan state has no jobs or no
  transaction job ending at the block's ledger. A submission is skipped when the
  jobs can't be listed. A produced block always goes alone.
- When a job is selected, the report waits for its fresh proof, and timestamps
  and signs the request after proving. A proving error skips the report; it does
  not send that block without work instead.

Uptime uses its own single-thread proving pool, separate from block production
and the general SNARK worker. Enabling uptime loads transaction and merge
circuits (including during dry run); zkApp circuits load on the first selected
zkApp job. This adds memory and CPU use. Loading and proof completion logs
include process RSS and timings. The worker does not enter the block producer's
queue, though both still share the machine's CPU and memory.

In a devnet measurement, loading uptime transaction and merge circuits added
about **560 MiB** of process RSS above a producer's already loaded circuits.
Loading zkApp circuits added another **707 MiB**, about **1.23 GiB** total above
the producer. These are circuit-loading measurements; allow additional memory
for proving and the node's normal work. Existing SNARK-worker circuits are
shared when already loaded.

### zkApp limitation

The zkApp path proves the first segment ending at the block's ledger and keeps
its all-zero SOK digest, exactly as OCaml 4.0.3/4.0.4 does. This is a known
upstream issue: the [uptime worker][zkapp-worker] leaves the digest at zero,
which its own [delegation verifier][zkapp-verifier] rejects. We will follow
upstream's fix. Ordinary coinbase and payment work still binds the submitter and
fee normally.

The local release gate records an exception only for zero-SOK work whose decoded
statement identifies an active zkApp execution ending in empty local state. It
still verifies the block, exact work encoding, ledger target and outer
signature. Other SOK mismatches and ambiguous statements fail. The exception
preserves the reference rejection; it is not a claim of full work verification
or service acceptance.

The
[uptime release note](https://github.com/minanode/mina-rust/blob/develop/docs/release/uptime-reporting.md)
records the exact memory measurements and intended differences from OCaml. These
include worker failure handling, flag aliases, key-directory permissions and the
User-Agent. It also describes the local release gate and the separate
owner-approved live acceptance plan.

Circuit-load failures retry on later requests with a bounded backoff. Uptime
failure does not stop the node. Local produced blocks remain reportable after a
fork removes them from the best chain.

## Node status

```sh
mina node --network mainnet --node-status-url https://status.example.org/
```

The first report goes five slots after the node starts, then one every five
slots. Each is OCaml's default simplified report, unsigned: the highest verified
block height received from peers, the full build commit, chain ID, peer ID,
number of connected peers, the time, and the block producer's public key (or
`null`). Any 2xx response succeeds; a report that fails is not retried, the next
one is a new sample.

`--simplified-node-stats` is `true` by default. `--simplified-node-stats false`
asks for OCaml's full report (version 2), which this node refuses: it doesn't
measure libp2p bandwidth or CPU, per-RPC and gossip counters, or the queues of
received blocks that report needs.

## Node errors

```sh
mina node --network mainnet --node-error-url https://errors.example.org/ \
  --contact-info 'ops@example.org'
```

A report is sent:

- for the first panic in the process, before the process goes on (it waits at
  most ten seconds for the report);
- each time transactions fail to apply to a block this node produces, with as
  many of them as fit.

Each report has its own ID and time. It is sent once, never retried, and not
merged with an equal earlier one. A report over 10,000 bytes is not sent. Up to
16 reports wait while one is being sent; more are dropped and counted. Reports
carry the node's peer ID, external IP, block producer public key (if any), build
commit, chain ID, sync status and heights. Private keys in an error message are
masked; hardware information is not collected.

`--contact-info TEXT` (shorter than 200 bytes) is included in node error reports
only.

## What the reporters did

`GET /reporting/status` on the node's HTTP port shows each running reporter
(`null` for one that isn't):

```json
{
  "uptime": {
    "target": "https://uptime.example.org/v1/submit",
    "report_kind": "uptime block and selected SNARK work",
    "reports": 12,
    "attempts": 14,
    "retries": 2,
    "delivered": 12,
    "rejected": 0,
    "dropped": 0,
    "interrupted": 0,
    "skipped": 1,
    "last_attempt_at": "2026-10-22T12:15:00Z",
    "last_success_at": "2026-10-22T12:15:00Z",
    "last_http_status": 200,
    "last_error": null,
    "last_skip_reason": "transition frontier not available",
    "last_report": {
      "kind": "best tip",
      "created_at": "2026-10-22T12:15:00Z",
      "chain_id": "…",
      "state_hash": "3N…",
      "height": 512345,
      "global_slot": 734567,
      "produced": false,
      "body_bytes": 912345,
      "id": null
    }
  },
  "node_status": null,
  "node_error": null
}
```

`retries` counts attempts after a report's first; `dropped` counts reports given
up after their last attempt or not sent (too large, queue full); `interrupted`
counts submissions stopped by the next one; `skipped` counts submissions not
made because the node wasn't ready. URLs are shown without user names, passwords
or query strings. The route is on the private HTTP router only: a node started
with `--public-graphql` doesn't serve it. The node's log has a line for each
attempt and outcome, with kind `Reporting`.

[zkapp-worker]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_snark_worker.ml#L86-L112
[zkapp-verifier]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/delegation_verify/delegation_verify.ml#L94-L123
