# mina command-line reference

Generated from the command definitions by `mina internal cli-reference --format markdown`. Do not edit: run that command, or the `mina-cli` test `cli_reference_is_current` with `MINA_UPDATE_CLI_REFERENCE=1`, to update it.

## mina

Mina node in Rust, with its wallet, ledger and operator tools.

```text
mina [OPTIONS] <COMMAND>
```

### Commands

- `node`: Run a Mina node, or inspect a running one with `mina node status`.
- `keys`: Generate or print account and libp2p key pairs.
- `build-info`: Print this binary's version, commit and build details.
- `wallet`: Create keys, check balances and send transactions through a node.
- `ledger`: Export a node's ledgers, and hash or total an exported ledger, as OCaml's `mina ledger`
- `fork`: Write the next hard fork's config from a running node, as OCaml's `mina advanced generate-hardfork-config`
- `report`: Collect a bug report and file it as a GitHub issue.
- `triage`: Let an AI agent draft the bug report issue from the redacted report.

### Global options

#### `--network <NETWORK>`

The network whose chain and keys the command uses.

Accepted by every command. Default: `devnet`. Environment: `MINA_NETWORK`.

Values:

- `devnet`
- `mainnet`

## mina node

Run a Mina node, or inspect a running one with `mina node status`.

Without options, the node joins the network given by `--network` as a full node: it finds peers, syncs, then follows the tip of the chain. The HTTP server serves the dashboard, the API and GraphQL on 127.0.0.1:3000. Options add block production, SNARK work and browser peers.

```text
mina node [OPTIONS] [COMMAND]
```

### Commands

- `status`: Inspect a running node using read-only HTTP requests.
- `frontier-store-check`: Check a frontier store directory offline, with the node stopped.

### Network

#### `-P, --peers <MULTIADDR>`

A peer to connect to at startup, as a multiaddr. Repeat it for more peers.

The format is `/ip4/<address>/tcp/<port>/p2p/<peer id>`, with `/ip6/` or `/dns4/` in place of `/ip4/` for other addresses. Peers given here, or by `--peer-list-file` or `--peer-list-url`, replace the network's default peers.

Alias: `--peer`.

#### `--peer-list-file <FILE>`

A file of peers to connect to at startup, one multiaddr per line.

Empty lines are skipped. Lines that don't parse are logged and skipped.

Environment: `MINA_PEER_LIST_FILE`.

#### `--peer-list-url <URL>`

A URL that returns peers to connect to at startup, in the `--peer-list-file` format.

Environment: `MINA_PEER_LIST_URL`.

#### `--seed`

Run as a seed node: without peers given, the node waits for peers to connect instead of dialling the network's default peers.

Environment: `MINA_SEED`.

#### `--max-peers <N>`

The most peer connections the node keeps.

Default: `100`.

#### `--libp2p-port <PORT>`

The TCP port for libp2p peer connections.

Alias: `--external-port`. Default: `8302`. Environment: `MINA_LIBP2P_PORT`.

#### `--libp2p-external-ip <IP>`

A public IP address at which peers may reach this node. Repeat it for more addresses.

#### `--libp2p-keypair <FILE>`

An encrypted libp2p key file, as OCaml's `mina libp2p generate-keypair` writes it, for the node's peer identity.

The password is read from the `MINA_LIBP2P_PASS` environment variable. The file takes precedence over `--p2p-secret-key`.

#### `-s, --p2p-secret-key <KEY>`

The node's libp2p secret key, for its peer identity.

By default the node reads `p2p-secret-key` in the work directory, and creates it on the first start.

Environment: `MINA_P2P_SEC_KEY`.

### HTTP and GraphQL

#### `-p, --port <PORT>`

The TCP port of the HTTP server: the dashboard, the API and GraphQL.

Default: `3000`. Environment: `MINA_PORT`.

#### `--http-addr <ADDR>`

The address the HTTP server listens on. `0.0.0.0` listens on all IPv4 interfaces.

Default: `127.0.0.1`. Environment: `MINA_HTTP_ADDR`.

#### `--http-cors-origin <ORIGIN>`

A page origin, like `https://dash.example.org`, whose pages may read this node's HTTP API besides pages of this machine.

Pages of this machine are `localhost`, `127.0.0.1` and `[::1]` on any port. Repeat the option, or give a comma-separated list in the environment; `*` lets every page read the API. Opening the node's pages at a host name rather than an IP address or `localhost` also needs that name listed, as `http://name:port`.

Environment: `MINA_HTTP_CORS_ORIGINS`.

#### `--enable-tx-submission`

Accept transactions over HTTP: `POST /send-payment` and the GraphQL mutations `sendPayment`, `sendDelegation` and `sendZkapp`.

Off by default, so that a public node does not take submissions from anyone who can reach its port. When off, they get HTTP 403 or a GraphQL error. Turn it on only for a node whose HTTP port only you can reach.

Environment: `MINA_ENABLE_TX_SUBMISSION`.

#### `--public-graphql`

Answer only a read-only GraphQL subset, for a node whose HTTP port anyone can reach.

The subset takes one query per request (no mutations, batches or introspection), with the fields block explorers, o1js and wallets read, `bestChain` with `maxLength` up to 20, and a cost ceiling. It leaves out the peers, SNARK pool, operator and telemetry fields. Refused requests get HTTP 400 before any work is done. The dashboard and GraphiQL are not served.

Environment: `MINA_PUBLIC_GRAPHQL`.

#### `--enable-ledger-export`

Serve whole-ledger exports over HTTP, for `mina ledger export` and `mina fork export`. Off by default: an export is expensive, so restrict access to the HTTP port first.

Environment: `MINA_ENABLE_LEDGER_EXPORT`.

#### `--no-dashboard`

Don't serve the dashboard at `/`. The API routes keep their paths.

Environment: `MINA_NO_DASHBOARD`.

#### `--metrics-port <PORT>`

Serve Prometheus metrics on this separate TCP port. Off by default.

Environment: `MINA_METRICS_PORT`.

#### `--metrics-addr <ADDR>`

The address the metrics listener listens on.

Default: `127.0.0.1`. Environment: `MINA_METRICS_ADDR`.

### Storage

#### `-d, --work-dir <DIR>`

The work directory: the node's chain state, caches, logs and settings.

Default: `~/.mina`. Environment: `MINA_HOME`.

#### `--no-frontier-store`

Don't keep a frontier store. The node keeps one by default, from which a restart resumes instead of syncing from the start.

The store holds the best chain's blocks as they come, and a checkpoint of the root's ledgers and staged ledger every `--checkpoint-interval` seconds. A restart restores from it when it passes its checks and isn't too old; otherwise the node bootstraps and logs why.

Environment: `MINA_NO_FRONTIER_STORE`.

#### `--frontier-store-no-restore`

Write the frontier store, but don't restore from it at startup.

#### `--frontier-store-dir <DIR>`

The directory for frontier stores, one per chain, at `<DIR>/<chain id>/frontier`. Defaults to the work directory.

Environment: `MINA_FRONTIER_STORE_DIR`.

#### `--frontier-store-reset`

Delete the frontier store when the node opens it, so that the node bootstraps.

#### `--frontier-store-load-timeout <SECONDS>`

Seconds a frontier store restore may go without progress before the node gives it up and bootstraps (1 to 300).

Default: `60`.

#### `--checkpoint-interval <SECONDS>`

Seconds between checkpoints of the frontier store's root.

Default: `1800`. Environment: `MINA_CHECKPOINT_INTERVAL`.

#### `--no-proof-cache`

Keep retained proofs in memory rather than in files under `<work dir>/proof-cache`. Takes precedence over `--proof-cache`.

Proofs go to the files by default, except on a block producer (`--producer-key`), which keeps the proofs it may read to build and prove a block in memory unless `--proof-cache` is given.

Environment: `MINA_NO_PROOF_CACHE`.

#### `--proof-cache`

Keep retained proofs in files under `<work dir>/proof-cache` on a block producer too. A failed read can then cost it a block.

#### `--no-ledger-disk`

Keep the staking, next epoch and root snarked ledgers' accounts in memory rather than in files under `<work dir>/ledger-accounts`

Environment: `MINA_NO_LEDGER_DISK`.

#### `--no-root-ledger-disk`

Keep the root snarked ledger's accounts in memory, while the epoch ledgers' go to files. No effect with `--no-ledger-disk`

Environment: `MINA_NO_ROOT_LEDGER_DISK`.

### Block production

#### `--producer-key <FILE>`

Produce blocks with the account key in this encrypted key file.

The password is read from the `MINA_PRIVKEY_PASS` environment variable; unset means an empty password.

Alias: `--block-producer-key`. Environment: `MINA_PRODUCER_KEY`.

#### `--coinbase-receiver <PUBLIC_KEY>`

The public key that receives the coinbase of the blocks this node produces. Defaults to the producer's own key.

If the key is a zkApp account's, its receive permission must be None.

#### `--block-inject-delay-ms <MILLISECONDS>`

The least milliseconds after the start of its slot before the node publishes a block it produced (at least 1000).

Default: `1000`.

### SNARK work

#### `--run-snarker <PUBLIC_KEY>`

Run the SNARK worker, with fees paid to this public key (B62...).

As with OCaml's `--run-snark-worker`, the worker needs no private key: the public key only goes into the message its proofs commit to. If the key is a zkApp account's, its receive permission must be None, or OCaml nodes reject the work.

Alias: `--run-snark-worker`. Environment: `MINA_RUN_SNARKER`.

#### `--snarker-fee <MINA>`

The fee asked for each SNARK work bundle, in MINA (for example `0.1`), as OCaml's `--snark-worker-fee`.

If the worker's key has no account yet, OCaml nodes only accept work whose fee is at least the account creation fee (1 MINA).

Alias: `--snark-worker-fee`. Default: `0.1`. Environment: `MINA_SNARKER_FEE`.

#### `--snarker-strategy <STRATEGY>`

How the SNARK worker picks its next job: `rand` picks any of the jobs it may take at random, as OCaml's default `--work-selection`, and `seq` the first in scan-state order.

It applies to a worker started with `--run-snarker`, from the dashboard, or from the setting the dashboard saved.

Default: `rand`. Environment: `MINA_SNARKER_STRATEGY`.

#### `--snarker-block-first-lead <SECONDS>`

Seconds before a won slot in which a node that also produces blocks starts no new SNARK job (0 to 4200; 0 turns it off). No effect without `--producer-key`

Default: `420`. Environment: `MINA_SNARKER_BLOCK_FIRST_LEAD`.

#### `--allow-worker-control`

Let the dashboard start, stop and re-price the SNARK worker (`POST /snarker/control`). Off by default.

The worker's settings decide where fees go, so allow this only on a node whose HTTP port only you can reach. It can't be combined with `--public-graphql`. A worker started from the dashboard is saved in `<work dir>/snark-worker.json` and comes back after a restart, with or without this option; `--run-snarker` takes precedence over it. Stopping the worker from the dashboard deletes the file, as does deleting it while the node is stopped.

Environment: `MINA_ALLOW_WORKER_CONTROL`.

#### `--worker-control-origin <ORIGIN>`

A page origin, like `https://dash.example.org`, that may change the SNARK worker besides pages of this machine.

Pages of this machine are `localhost`, `127.0.0.1` and `[::1]` on any port. Repeat the option, or give a comma-separated list in the environment. Requests from any other page are refused.

Environment: `MINA_WORKER_CONTROL_ORIGINS`.

### WebRTC

#### `--webrtc-signal-port <PORT>`

Serve WebRTC signaling for browser peers on this separate TCP port. Off by default.

Environment: `MINA_WEBRTC_SIGNAL_PORT`.

#### `--webrtc-signal-addr <ADDR>`

The address the signaling listener listens on. Pages served over HTTPS need a TLS proxy in front of it.

Default: `127.0.0.1`. Environment: `MINA_WEBRTC_SIGNAL_ADDR`.

#### `--webrtc-ice-servers <FILE>`

A JSON file of ICE servers for WebRTC connections, in the browser's `RTCIceServer` list format. Defaults to the built-in STUN servers.

For example `[{"urls": "stun:stun.example.org:3478"}]`, or `[]` for none. A node with a public address needs none. A file keeps TURN credentials off the command line.

Environment: `MINA_WEBRTC_ICE_SERVERS`.

#### `--webrtc-udp-ports <MIN-MAX>`

The UDP ports for WebRTC candidates, as `MIN-MAX`, so that a firewall need open only those. Defaults to any port.

Each connection takes a port per local address: allow several per `--max-peers`.

Environment: `MINA_WEBRTC_UDP_PORTS`.

#### `--webrtc-nat-ip <IP>`

The public address of a 1:1 NAT in front of this node, used in its WebRTC host candidates in place of its interface addresses.

For a cloud host whose interface has a private address. Repeat it, or separate addresses with commas, for IPv4 and IPv6.

Environment: `MINA_WEBRTC_NAT_IP`.

### Logging

#### `-v, --verbosity <LEVEL>`

The log level of the console and the log file: `error`, `warn`, `info`, `debug` or `trace`.

`info` logs what an operator follows: the settings at startup, sync progress, each new best tip, a status line with the peer count, the blocks produced, the SNARK work sold, warnings and errors. `debug` adds detail for diagnosis, and `trace` adds every action of the state machine.

Alias: `--log-level`. Default: `info`. Environment: `MINA_VERBOSITY`.

#### `--disable-filesystem-logging`

Log to the console only, without `mina.log`

Environment: `MINA_DISABLE_FILESYSTEM_LOGGING`.

#### `--log-path <DIR>`

The directory for the log file, `mina.log`. Defaults to the work directory.

Environment: `MINA_LOG_PATH`.

#### `--log-file-size-mb <MIB>`

The size in MiB at which `mina.log` is rotated to `mina.log.1`, older files shifting up one.

Default: `20`. Environment: `MINA_LOG_FILE_SIZE_MB`.

#### `--log-file-count <N>`

The number of log files kept, `mina.log` included; the oldest is deleted. The logs take at most `--log-file-size-mb` times this.

Default: `10`. Environment: `MINA_LOG_FILE_COUNT`.

### Advanced

#### `-c, --config <FILE>`

A daemon config JSON file to start from instead of the network's embedded genesis, as OCaml's `--config-file`

Environment: `MINA_CONFIG`.

#### `--genesis-config <FILE>`

A genesis config to start from instead of `--config`'s.

It is either a prebuilt config as `ledger-tool` writes it (bin_prot), or one written with `--without-ledgers` (JSON). The latter only suits a network that has moved past its genesis, as the ledgers are synced from peers.

Environment: `MINA_GENESIS_CONFIG`.

#### `--no-peers-discovery`

Turn off peer discovery: no Kademlia, and no asking peers for more peers.

The node still dials its initial peers. Those are the network's default peers unless peers are given with `--peers`, `--peer-list-file` or `--peer-list-url`, or `--seed` is set.

## mina node status

Inspect a running node using read-only HTTP requests.

```text
mina node status [OPTIONS]
```

### Options

#### `--node <URL>`

The running node's HTTP base URL. The report names the network that node is on.

Default: `http://127.0.0.1:3000`. Environment: `MINA_NODE_URL`.

#### `--status-only`

Use only GET /status; omit build and verification statistics.

#### `--json`

Print a JSON report, also when a request fails.

#### `--check`

Exit with status 1 if the node is not synced, its best tip is missing or older than `--max-tip-age`, or it has fewer peers than `--min-peers`

#### `--max-tip-age <SECONDS>`

The oldest best tip `--check` accepts, in seconds.

Default: `1800`.

#### `--min-peers <N>`

The fewest connected peers `--check` accepts; peers still connecting don't count.

Default: `1`.

#### `--timeout <SECONDS>`

Seconds to wait for each HTTP request (1 to 300).

Default: `5`.

## mina node frontier-store-check

Check a frontier store directory offline, with the node stopped.

Loads the newest valid generation as a restart would, rehashing every ledger by default, and checks the checkpoint's root staged ledger against its block, the parts' protocol states and the chain's blocks. Writes nothing. Exits with an error if a check fails. A running node locks its store: stop it first, or pass `--no-lock`.

The store is `<chain id>/frontier` in the work directory, `/data` in Docker.

Example: `mina node frontier-store-check ~/.mina/<chain id>/frontier`

```text
mina node frontier-store-check [OPTIONS] <DIR>
```

### Arguments

#### `<DIR>`

The store: a chain's `frontier` directory.

Required.

### Options

#### `--verify <VERIFY>`

How much of each ledger to rehash: `full` (every node), `sample` (64 leaf paths, as a restart does) or `root`

Default: `full`.

Values:

- `full`
- `sample`
- `root`

#### `--rebuild-staged`

Also rebuild the root staged ledger the slow way, by applying the scan state's transactions again (tens of seconds on mainnet).

#### `--no-lock`

Check without taking the store's lock, while a node writes it.

## mina keys

Generate or print account and libp2p key pairs.

```text
mina keys [OPTIONS] <COMMAND>
```

### Commands

- `mina-key-pair`: Generate an unencrypted Mina account key pair, and print it.
- `p2p-key-pair`: Generate a libp2p key pair for a node's peer identity, and print it.

## mina keys mina-key-pair

Generate an unencrypted Mina account key pair, and print it.

Prints the secret key and the public key. For a key file a block producer can use, run `mina wallet generate` instead.

```text
mina keys mina-key-pair [OPTIONS]
```

### Options

#### `-s, --secret-key <KEY>`

Print the key pair of this secret key instead of a new one.

Environment: `MINA_SEC_KEY`.

## mina keys p2p-key-pair

Generate a libp2p key pair for a node's peer identity, and print it.

Prints the secret key, the public key and the peer id. The secret key goes to `mina node --p2p-secret-key`.

```text
mina keys p2p-key-pair [OPTIONS]
```

### Options

#### `-s, --p2p-secret-key <KEY>`

Print the key pair of this secret key instead of a new one.

Environment: `MINA_P2P_SEC_KEY`.

## mina build-info

Print this binary's version, commit and build details.

```text
mina build-info [OPTIONS]
```

## mina wallet

Create keys, check balances and send transactions through a node.

```text
mina wallet [OPTIONS] <COMMAND>
```

### Commands

- `address`: Print the public key of an encrypted key file.
- `balance`: Print an account's balance and nonce, from a node's GraphQL API.
- `delegate`: Delegate an account's stake, as OCaml's `mina client delegate-stake`
- `generate`: Generate an account key pair and save it as an encrypted key file.
- `send`: Sign a payment and send it through a node.
- `status`: Print a transaction's status, from a node's GraphQL API, with a pooled transaction's amount and fee in MINA.

## mina wallet address

Print the public key of an encrypted key file.

```text
mina wallet address [OPTIONS] --from <FILE>
```

### Options

#### `--from <FILE>`

The encrypted key file. Its password is read from the `MINA_PRIVKEY_PASS` environment variable.

Required. Environment: `MINA_FROM`.

## mina wallet balance

Print an account's balance and nonce, from a node's GraphQL API.

```text
mina wallet balance [OPTIONS]
```

### Options

#### `--address <PUBLIC_KEY>`

The account's public key.

#### `--from <FILE>`

An encrypted key file whose account to query, in place of `--address`. Its password is read from the `MINA_PRIVKEY_PASS` environment variable.

#### `--node <URL>`

The node's HTTP base URL (its `--port`). Requests go to `<URL>/graphql`

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

#### `--format <FORMAT>`

The output format.

Default: `text`.

Values:

- `text`
- `json`

## mina wallet delegate

Delegate an account's stake, as OCaml's `mina client delegate-stake`

```text
mina wallet delegate [OPTIONS] --from <FILE> --to <PUBLIC_KEY> --fee <MINA>
```

### Options

#### `--from <FILE>`

The encrypted key file of the account delegating its stake. Its password is read from `MINA_PRIVKEY_PASS`, or asked for on a terminal.

Required.

#### `--to <PUBLIC_KEY>`

The public key to delegate the stake to, as OCaml's `--receiver`

Required.

#### `--fee <MINA>`

The transaction fee in MINA, like `0.1`, with at most 9 decimal places, as OCaml's `--fee`

Required.

#### `--nonce <NONCE>`

The nonce. Defaults to the node's inferred nonce for the sender, which counts its pending transactions.

#### `--memo <TEXT>`

The memo, at most 32 bytes.

#### `--valid-until <SLOT>`

The last global slot since genesis in which the delegation may be included. By default it never expires.

#### `--node <URL>`

The node's HTTP address (its `--port`). The node must run with `--enable-tx-submission`

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

#### `--format <FORMAT>`

The output format.

Default: `text`.

Values:

- `text`
- `json`

## mina wallet generate

Generate an account key pair and save it as an encrypted key file.

```text
mina wallet generate [OPTIONS] --output <FILE>
```

### Options

#### `--output <FILE>`

The encrypted key file to write. Neither it nor its `.pub` file may exist.

Required.

#### `-s, --secret-key <KEY>`

Encrypt this secret key instead of a new one.

Environment: `MINA_SEC_KEY`.

## mina wallet send

Sign a payment and send it through a node.

```text
mina wallet send [OPTIONS] --from <FILE> --to <PUBLIC_KEY> --amount <MINA> --fee <MINA>
```

### Options

#### `--from <FILE>`

The sender's encrypted key file. Its password is read from the `MINA_PRIVKEY_PASS` environment variable.

Required. Environment: `MINA_FROM`.

#### `--to <PUBLIC_KEY>`

The receiver's public key.

Required.

#### `--amount <MINA>`

The amount in MINA, like `2.5`, with at most 9 decimal places, as OCaml's `mina client send-payment --amount`

Required.

#### `--fee <MINA>`

The transaction fee in MINA, like `0.1`, with at most 9 decimal places.

Required.

#### `--memo <TEXT>`

The memo, at most 32 bytes.

#### `--nonce <NONCE>`

The nonce. Defaults to the sender account's nonce in the node's ledger.

#### `--valid-until <SLOT>`

The last global slot since genesis in which the payment may be included. By default it never expires.

#### `--node <URL>`

The node's HTTP base URL (its `--port`). The node must run with `--enable-tx-submission`

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

## mina wallet status

Print a transaction's status, from a node's GraphQL API, with a pooled transaction's amount and fee in MINA.

```text
mina wallet status [OPTIONS] --hash <HASH>
```

### Options

#### `--hash <HASH>`

The transaction hash.

Required.

#### `--node <URL>`

The node's HTTP base URL (its `--port`).

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

#### `--check-mempool`

Leave out the hint to use this option when the status is UNKNOWN.

It doesn't change the lookup: the transaction pool is searched only when the node answers the status query with an error.

## mina ledger

Export a node's ledgers, and hash or total an exported ledger, as OCaml's `mina ledger`

```text
mina ledger [OPTIONS] <COMMAND>
```

### Commands

- `export`: Write a ledger of a running node's best tip, in OCaml-compatible JSON (compact account lines, rather than OCaml's pretty-printed whitespace).
- `hash`: Print the Merkle root of the ledger in an exported ledger file.
- `currency`: Print the total currency of each token in an exported ledger file.

## mina ledger export

Write a ledger of a running node's best tip, in OCaml-compatible JSON (compact account lines, rather than OCaml's pretty-printed whitespace).

```text
mina ledger export [OPTIONS] <KIND>
```

### Arguments

#### `<KIND>`

Which ledger of the node's best tip.

Required.

Values:

- `staged-ledger`: The best tip's staged ledger.
- `snarked-ledger`: The best tip's snarked ledger.
- `staking-epoch-ledger`: The ledger the current epoch stakes from.
- `next-epoch-ledger`: The ledger the next epoch will stake from.

### Options

#### `--node <URL>`

The node's HTTP address (its `--port`). The node must run with `--enable-ledger-export`

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

#### `-o, --output <FILE>`

Write the accounts to this file instead of standard output; a summary goes to standard output.

#### `--verify`

Recompute the written file's Merkle root and exit with status 4 unless it is the node's (needs `--output`).

## mina ledger hash

Print the Merkle root of the ledger in an exported ledger file.

```text
mina ledger hash [OPTIONS] --ledger-file <FILE>
```

### Options

#### `--ledger-file <FILE>`

A ledger file as `mina ledger export` writes it (JSON).

Required.

#### `--expect <HASH>`

Exit with status 4 unless the Merkle root is this ledger hash.

#### `--json`

Print a JSON object (`ledger_hash`, `num_accounts`) instead of the hash.

## mina ledger currency

Print the total currency of each token in an exported ledger file.

```text
mina ledger currency [OPTIONS] --ledger-file <FILE>
```

### Options

#### `--ledger-file <FILE>`

A ledger file as `mina ledger export` writes it (JSON).

Required.

## mina fork

Write the next hard fork's config from a running node, as OCaml's `mina advanced generate-hardfork-config`

```text
mina fork [OPTIONS] <COMMAND>
```

### Commands

- `export`: Write the next hard fork's runtime config and ledgers from a running node's state at the stop slots.

## mina fork export

Write the next hard fork's runtime config and ledgers from a running node's state at the stop slots.

```text
mina fork export [OPTIONS] --out <DIR>
```

### Options

#### `--node <URL>`

The node's HTTP address (its `--port`). The node must run with `--enable-ledger-export`

Default: `http://localhost:3000`. Environment: `MINA_NODE_URL`.

#### `--out <DIR>`

Directory to create. It must not exist; it gets `fork_config.json` (every account inline, OCaml's GraphQL `fork_config` layout), `daemon.json` and `daemon-migrated.json` (ledgers by stable and by OCaml 4.0.2's migrated root), `manifest.json` and, last, an empty `activated`

Required.

#### `--state-hash <HASH>`

Continue from this best-chain block instead of the newest block before `slot_tx_end`

#### `--height <HEIGHT>`

Continue from the best-chain block at this height.

#### `--allow-before-stop`

Export even if the best tip has not reached slot_tx_end, so that the fork block may still change.

#### `--verify`

Recompute the three ledgers' stable and migrated roots from fork_config.json and exit with status 4 unless they are the node's.

## mina report

Collect a bug report and file it as a GitHub issue.

Gathers, without AI: this binary's version and build; the OS, kernel,
CPU (and whether it has BMI2 and ADX), memory and free disk; what the
running node says (network, sync, best and highest peer tip, peers,
producer and worker); the newest crash file the node wrote; warnings,
errors and panics from the end of its log; a check of its frontier store;
and the command line it was started with.

Keys, passwords, key files and their paths, and secret environment
variables are never included; IP addresses and peer ids only with
`--include-network-ids`.

The report is printed first. Once you confirm (or with `--yes`), a
pre-filled issue opens in your browser, where you review and submit it;
with `--gh`, `gh issue create` files it. `--output FILE` only saves it.

Examples:

mina report
  mina report -d ~/.mina-devnet -m "stops syncing after an hour"
  journalctl --user -u mina | mina report --log -
  mina report --output report.md.

```text
mina report [OPTIONS]
```

### Options

#### `-d, --work-dir <DIR>`

The node's work directory: `last-start.json`, crash files, logs and the frontier store are read from it.

Default: `~/.mina`. Environment: `MINA_HOME`.

#### `--node <URL>`

The running node's HTTP base URL. Default: the port the node last started with (from `last-start.json`), else http://127.0.0.1:3000.

Environment: `MINA_NODE_URL`.

#### `--log <FILE>`

A log file to scan (repeatable); `-` reads standard input, as in `journalctl --user -u mina | mina report --log -`. By default, scan `mina.log` and its numbered rotations, newest first, within 8 MiB.

#### `--log-lines <N>`

How many warning, error and panic lines to include (1 to 1000).

Default: `100`.

#### `--store-check <STORE_CHECK>`

How to check the frontier store: `root` (fast), `sample`, `full` (rehashes every ledger) or `off`

Default: `root`.

Values:

- `root`
- `sample`
- `full`
- `off`

#### `--include-network-ids`

Keep IP addresses and peer ids in the report (left out by default).

#### `-m, --message <TEXT>`

What happened, in a sentence or two. Asked for when not given and the terminal is interactive.

#### `-o, --output <FILE>`

Only save the report to FILE: nothing is sent.

#### `-y, --yes`

File without asking first.

#### `--repo <OWNER/NAME>`

The GitHub repository to file the issue in.

Default: `minanode/mina-rust`.

#### `--gh`

Create the issue with the GitHub CLI (`gh`), if installed, instead of opening a pre-filled issue page.

#### `--no-browser`

Print the pre-filled issue link instead of opening a browser.

#### `--timeout <SECONDS>`

Seconds to wait for each answer from the node (1 to 120).

Default: `5`.

## mina triage

Let an AI agent draft the bug report issue from the redacted report.

Collects the report `mina report` would, then runs the `claude` CLI, if
installed, non-interactively in a new temporary directory that holds only
the redacted report and the redacted last 2000 lines of each log. Claude
runs in its --restricted mode with only Read, Grep and Glob: it can't run
commands, use the web, or read anything outside that directory, and gets
only the environment variables it needs to sign in. Its draft is shown,
and filed as `mina report` files, only once you confirm.

What it is given goes to Anthropic: you are asked before it runs. codex
can't be confined that way (it could read any file you can read), so it
runs only with --agent codex --allow-unconfined-agent. Without an agent,
`mina triage` says so and files the plain report.

Examples:

mina triage
  mina triage --model claude-opus-5-5.

```text
mina triage [OPTIONS]
```

### Options

#### `-d, --work-dir <DIR>`

The node's work directory: `last-start.json`, crash files, logs and the frontier store are read from it.

Default: `~/.mina`. Environment: `MINA_HOME`.

#### `--node <URL>`

The running node's HTTP base URL. Default: the port the node last started with (from `last-start.json`), else http://127.0.0.1:3000.

Environment: `MINA_NODE_URL`.

#### `--log <FILE>`

A log file to scan (repeatable); `-` reads standard input, as in `journalctl --user -u mina | mina report --log -`. By default, scan `mina.log` and its numbered rotations, newest first, within 8 MiB.

#### `--log-lines <N>`

How many warning, error and panic lines to include (1 to 1000).

Default: `100`.

#### `--store-check <STORE_CHECK>`

How to check the frontier store: `root` (fast), `sample`, `full` (rehashes every ledger) or `off`

Default: `root`.

Values:

- `root`
- `sample`
- `full`
- `off`

#### `--include-network-ids`

Keep IP addresses and peer ids in the report (left out by default).

#### `-m, --message <TEXT>`

What happened, in a sentence or two. Asked for when not given and the terminal is interactive.

#### `-o, --output <FILE>`

Only save the report to FILE: nothing is sent.

#### `-y, --yes`

File without asking first.

#### `--repo <OWNER/NAME>`

The GitHub repository to file the issue in.

Default: `minanode/mina-rust`.

#### `--gh`

Create the issue with the GitHub CLI (`gh`), if installed, instead of opening a pre-filled issue page.

#### `--no-browser`

Print the pre-filled issue link instead of opening a browser.

#### `--timeout <SECONDS>`

Seconds to wait for each answer from the node (1 to 120).

Default: `5`.

#### `--agent <AGENT>`

The agent to run; asked for when both are installed.

Values:

- `claude`
- `codex`

#### `--model <MODEL>`

The model, passed to the agent's `--model`

#### `--agent-timeout <MINUTES>`

Minutes the agent may take (1 to 120).

Default: `15`.

#### `--allow-unconfined-agent`

Run an agent that can't be kept to the redacted report: codex, whose read-only sandbox limits writes, not reads. It can read any file you can read, key files included, and send it to OpenAI.
