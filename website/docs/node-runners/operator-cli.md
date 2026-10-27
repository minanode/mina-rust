---
title: Ledger export and stake delegation
sidebar_position: 2
description:
  mina ledger export, hash and currency, and mina wallet delegate, the
  equivalents of OCaml's ledger and delegate-stake commands.
---

# Ledger export and stake delegation

These commands do what OCaml operators do with `mina ledger export`,
`mina ledger hash`, `mina ledger currency` and `mina client delegate-stake`.
They talk to a running node over its HTTP port (`--port`, default 3000), the
same listener that serves GraphQL at `/graphql`. Keep that port private, as the
quick start explains. Ledger export is off by default; enable it explicitly with
`mina node --enable-ledger-export` (or `MINA_ENABLE_LEDGER_EXPORT=true`) only
after restricting access to that port. With the flag, anyone who reaches it can
export ledgers. `--enable-tx-submission` separately enables transaction
submission. Public GraphQL mode never mounts the export routes, even if export
is enabled.

## Units

- **Fees** on the command line are in **MINA**, with at most 9 decimal places,
  as in OCaml. A 0.1 MINA fee is `--fee 0.1`.
- **Balances in ledger files** are in **MINA**, as decimal strings, exactly as
  OCaml writes them: `"1000"`, `"0.000001"`, `"49460.00155731"`.
- **Slots** (`--valid-until`, `cliff_time`) are global slots since genesis.

## Network and node

Every command that talks to a node takes:

- `--network devnet|mainnet` (or `MINA_NETWORK`; default devnet). The command
  checks the node's `networkID` and stops if it is on the other network.
  Delegations are signed for this network: a devnet signature is invalid on
  mainnet and the reverse.
- `--node URL` (or `MINA_NODE_URL`; default `http://localhost:3000`): the node's
  HTTP endpoint, without `/graphql`.

## Exit status

| Status | Meaning                                                                             |
| ------ | ----------------------------------------------------------------------------------- |
| 0      | Done                                                                                |
| 1      | Failed: bad input, unreadable key file, node unreachable, wrong network, not synced |
| 2      | Bad command line (unknown option, missing argument)                                 |
| 3      | The node refused: a rejected delegation, a ledger it cannot export, a busy export   |
| 4      | A recomputed ledger hash differs from the expected one (`--expect`, `--verify`)     |

Errors go to standard error, prefixed `[ERROR]`.

## Export a ledger

Start your operator node with `--enable-ledger-export`. A disabled export
returns HTTP 403 and the CLI exits with status 3, with instructions to enable
the flag. Public GraphQL endpoints return 404; use the private operator
endpoint.

```bash
mina ledger export staking-epoch-ledger --network devnet \
  --node http://localhost:3000 --output staking.json --verify
```

The ledger is one of:

| Ledger                 | What it is                                     |
| ---------------------- | ---------------------------------------------- |
| `staged-ledger`        | the best tip's ledger, with all its blocks     |
| `snarked-ledger`       | the best tip's ledger as far as it is proved   |
| `staking-epoch-ledger` | the ledger the current epoch's stake is from   |
| `next-epoch-ledger`    | the ledger the next epoch's stake will be from |

The file is JSON- and hash-compatible with OCaml's `mina ledger export`: a JSON
array of accounts in ledger index order, in the runtime-config layout (`pk`,
`balance`, `delegate`, `timing`, `token`, `nonce`, `receipt_chain_hash`,
`voting_for`, `zkapp`, `permissions`, `token_symbol`; fields at their default
are left out). OCaml's `mina ledger hash` and runtime configs read it. Rust
writes one compact account per line; OCaml pretty-prints, so the files are not
byte identical. Without `--output` the accounts go to standard output.

The command prints a JSON summary, to standard output with `--output` and to
standard error without it:

```json
{
  "kind": "staking-epoch-ledger",
  "ledger_hash": "jx...",
  "num_accounts": 289012,
  "state_hash": "3N...",
  "blockchain_length": 580123,
  "global_slot_since_genesis": 812345,
  "epoch": 113,
  "network_id": "mina:devnet",
  "sync_status": "SYNCED",
  "bytes": 291205907,
  "output": "staking.json",
  "recomputed_ledger_hash": "jx..."
}
```

`ledger_hash` is the Merkle root the node computed for the ledger it exported.
For `staking-epoch-ledger` and `next-epoch-ledger` it is the ledger hash in the
best tip's consensus state, and for `snarked-ledger` its blockchain state's
snarked ledger hash; the node refuses (status 3) rather than export another.
`--verify` recomputes the root from the written file and exits with status 4 if
it differs.

A mainnet ledger is about 300 MB of JSON; the export takes seconds to a minute,
and the node serves one export at a time through the end of the download.
Another request receives HTTP 503 while an export is being built or delivered; a
disconnect releases the permit after any pending ledger RPC finishes. The node
retains one whole serialized ledger and sends it in chunks of at most 64 KiB,
without making another ledger-sized copy for HTTP. Ledger-thread work and its
account snapshot still scale with ledger size. The node needs a best tip; while
it bootstraps, the export is refused.

The same file comes straight from the node's HTTP API, with the summary in
`x-mina-*` response headers:

```bash
curl -fsS -D headers.txt -o staged.json \
  http://localhost:3000/ledger/export/staged-ledger
```

## Check a ledger file

```bash
mina ledger hash --ledger-file staking.json
mina ledger hash --ledger-file staking.json --expect jx... --json
mina ledger currency --ledger-file staking.json
```

`hash` prints the Merkle root of a ledger holding the file's accounts in order,
as OCaml's `mina ledger hash --ledger-file` does, and works offline on OCaml's
exports too. `--expect HASH` exits with status 4 unless the root is `HASH`;
`--json` prints `{"ledger_hash": ..., "num_accounts": ...}`. `currency` prints
the total balance of each token, `MINA: <total>` first, as OCaml does.

To check a staking ledger against the chain, compare its hash with the best
tip's `stakingEpochData.ledger.hash` in GraphQL, or with any OCaml node's.

## Delegate stake

```bash
export MINA_PRIVKEY_PASS='...'   # the key file's password
mina wallet delegate --network devnet --node http://localhost:3000 \
  --from my-key.json --to B62q... --fee 0.1 --format json
```

| Option          | Meaning                                                          |
| --------------- | ---------------------------------------------------------------- |
| `--from`        | encrypted key file of the delegating account                     |
| `--to`          | public key to delegate to (`--receiver` also works, as in OCaml) |
| `--fee`         | fee in MINA, like `0.1`                                          |
| `--nonce`       | default: the node's inferred nonce, which counts pending ones    |
| `--memo`        | at most 32 bytes                                                 |
| `--valid-until` | last global slot it may be included in; default: never expires   |
| `--format`      | `text` (default) or `json`                                       |

The command checks that the node is on the network and synced, decrypts the key
file, signs the delegation locally as OCaml signs it, and submits it through the
node's GraphQL `sendDelegation`. The node must run with
`--enable-tx-submission`. The password comes only from `MINA_PRIVKEY_PASS` or a
terminal prompt, never from the command line, and the secret key is never sent
or printed: the node receives the signed delegation.

JSON output:

```json
{
  "status": "pending",
  "hash": "5J...",
  "id": "...",
  "from": "B62q...",
  "to": "B62q...",
  "fee_nanomina": "100000000",
  "nonce": 4,
  "valid_until": 4294967295,
  "memo": "",
  "node": "http://localhost:3000"
}
```

`pending` means the node accepted it into its pool. Follow it with
`mina wallet status --hash <hash>`, and confirm the new delegate in the
account's `delegateAccount { publicKey }` once a block includes it. The new
delegate gets the stake from the staking ledger of the epoch after next, as in
OCaml.

## Keys

`mina wallet generate --output my-key.json` makes an encrypted key file
(`my-key.json.pub` holds the public key), and
`mina wallet address --from my-key.json` prints its public key; both read the
password from `MINA_PRIVKEY_PASS`. There is no key store in the node: each
command reads the key file it is given.
