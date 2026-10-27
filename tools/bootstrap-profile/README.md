# Bootstrap profiling

Scripts used for R4 to break a native bootstrap from mainnet peers into phases.

- `run-node.sh BINARY OUTDIR HTTP_PORT LIBP2P_PORT` starts a node from empty
  that joins mainnet, with the genesis config without ledgers from
  `$ROOT/.work/genesis-mainnet`, logging to `OUTDIR/node.log`, and starts
  `sample.py` against it. `ROOT` defaults to the main checkout of the repository
  the script is in, found with `git rev-parse --git-common-dir`, so it works
  from a worktree. The node keeps running: kill it by the PID in `OUTDIR/pid`.
- `sample.py` polls `/status`, `/state/peers`, `/state/message-progress` and
  `/stats/sync` every 2 s, with per-thread CPU from `/proc` and per-socket
  received bytes from `ss`. It writes `samples.jsonl`, then `final-*.json` once
  the node has been synced for a minute.
- `analyse.py LOG [OUTDIR]` prints the phases, errors by kind, disconnection
  reasons, and, with `patches/rpc-logging.patch` applied to the node, RPCs per
  phase with latencies and peers.
- `perpeer.py`, `querylevels.py`, `sessions.py`, `tips.py`, `bytes.py`,
  `peers_detail.py` and `actions.py` answer narrower questions; each says what
  it does at the top.

Every Python script takes `--help`. The R4 runs' logs and samples are in
`$ROOT/logs/r4` (git-ignored), for example:

```sh
tools/bootstrap-profile/analyse.py $ROOT/logs/r4/run5/node.log $ROOT/logs/r4/run5
```

`patches/` holds the prototype node changes the runs used, none of them merged:

- `rpc-logging.patch` logs every outgoing RPC request, response and timeout with
  its peer and kind, the peer and error in the sync error logs, and the tree
  level and size of each ledger sync answer (about 30,000 extra log lines per
  bootstrap). `perpeer.py`, `querylevels.py`, `sessions.py` and the RPC sections
  of `analyse.py` need it.
- `query-depth-7.patch` sets `SYNC_LEDGER_QUERY_DEPTH` to 7 (R4 run 5).
- `reducer-only-depth.patch` changes only the reducer's query depth, which
  stalls ledger sync because the answers are then rejected (R4 run 6).

Apply them with `git apply` on top of `main` in a worktree of your own, the
logging first; either depth patch then applies on top of it.

Use your own ports (see the historical baseline) and stop the node when done. On
the shared development machine, start the node in one of the two node slots,
which it inherits and holds until it exits (`tools/regression/README.md`,
"Taking turns"):

```sh
tools/regression/lock.sh node -- \
  tools/bootstrap-profile/run-node.sh BINARY OUTDIR HTTP_PORT LIBP2P_PORT
```

`lock.sh status` then names `run-node.sh`'s PID, which has exited; the node's is
in `OUTDIR/pid`.
