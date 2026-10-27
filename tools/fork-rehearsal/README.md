# Hard-fork rehearsal (R43f)

An isolated rehearsal of the next Mina hard fork with OCaml 4.0.2 and the Rust
node: both run an old chain to its stop slots, both write the next chain's
config from their own state, the configs are compared field by field and the
ledgers by hash, and each node then starts the next chain from its own export.
Results: R43f.

Both nodes run in an unprivileged, loopback-only user/network namespace
(`tools/local-mesa-net/namespace.py`), with throwaway keys and Mesa devnet
constants, circuits and fork. No node slot is needed; builds and the circuit
dump take the build lock. Check the host load first and wait while the 1-minute
load is over 40. OCaml commands use the run's private `TMPDIR`.

## Old chain

Build `mina`, the `local_mesa_identity` example (mina-block-replay) and the
`circuit_digests` example (mina-tree), and copy them to a private binary
directory. `BLOBS` holds the Mesa devnet circuits (`4.0.2devnet`, P4.8).

```bash
RUN=/tmp/r43f/run1
MINA=/tmp/r43f/bin/mina
BLOBS=/tmp/p4.8/gen/circuit-blobs
python3 tools/fork-rehearsal/prepare.py --dir "$RUN" --mina "$MINA" \
  --ocaml scripts/ocaml-mina --genesis-delay 600 \
  --slot-tx-end 10 --slot-chain-end 14 --genesis-slot-delta 12
python3 tools/local-mesa-net/namespace.py --dir "$RUN" > "$RUN/logs/namespace.log" 2>&1 &
```

`prepare.py` makes a producer for each node (Rust 3,000,000 MINA; OCaml
1,000,000), payment accounts, an account delegating to Rust's producer and a
timed account, with different genesis, staking and next epoch ledgers. Compute
and match both nodes' genesis identities as in
[local-mesa-net](../local-mesa-net/README.md) (`local_mesa_identity`, OCaml
`internal chain-id` and `test-genesis-block-generation`, `match_identity.py`).
Then start OCaml (follower) and Rust (the old chain's producer):

```bash
python3 tools/fork-rehearsal/launch.py ocaml --dir "$RUN" --phase old \
  > "$RUN/logs/ocaml-old.log" 2>&1 &
python3 tools/fork-rehearsal/launch.py rust --dir "$RUN" --phase old \
  --mina "$MINA" --blobs "$BLOBS" --producer producer > "$RUN/logs/rust-old.log" 2>&1 &
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/fork-rehearsal/sample.py \
  --dir "$RUN" --phase old --seconds 3000 &
```

Before `slot_tx_end`, send transactions through `tx.py` (payments with nonces 0,
1, ...; a delegation). After it, a payment must be refused.

## The stop and the exports

From the start of `slot_chain_end` (no block can be added), run both exports and
compare them:

```bash
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/fork-rehearsal/fork_export.py \
  --dir "$RUN" --mina "$MINA"
python3 tools/fork-rehearsal/new_chain.py --dir "$RUN" --identity "$IDENTITY" \
  --digests "$DIGESTS"            # also writes new/ocaml-base.json
python3 tools/fork-rehearsal/compare.py --dir "$RUN"
```

`fork_export.py` runs `mina fork export --verify` against Rust and both OCaml
procedures (`advanced generate-hardfork-config`, GraphQL `fork_config`).
`new_chain.py` dumps the new fork's circuits with OCaml (`generate.sh`, with
`MESA_CIRCUITS_LIVE_KEY=none`), saves the chain id's digests traced from OCaml
(`scripts/ocaml-chain-id-trace.sh`) next to them as `chain-id-digests.txt`,
binds that trace and all 24 files to the dumped fork with
`tools/mesa-circuits/bind-fork-bundle.py` (`fork-bundle.json`), then computes
Rust's new genesis and chain id from `fork/rust/fork_config.json` and OCaml's
from its own `fork_validation/legacy` output, generating a full genesis proof,
and requires them to agree. `compare.py` writes `fork/comparison.json`.

## Supervised restart onto the new chain

Stop the old nodes (SIGTERM; their state directories `rust-old/`, `ocaml-old/`
are kept), then start each node from its own export before the new genesis time:

```bash
python3 tools/fork-rehearsal/stop.py --dir "$RUN" --name rust-old
python3 tools/fork-rehearsal/stop.py --dir "$RUN" --name ocaml-old
python3 tools/fork-rehearsal/launch.py ocaml --dir "$RUN" --phase new \
  --producer ocaml-producer > "$RUN/logs/ocaml-new.log" 2>&1 &
python3 tools/fork-rehearsal/launch.py rust --dir "$RUN" --phase new --mina "$MINA" \
  --blobs "$RUN/new/circuits/circuit-blobs" --producer producer \
  > "$RUN/logs/rust-new.log" 2>&1 &
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/fork-rehearsal/sample.py \
  --dir "$RUN" --phase new --seconds 2400 &
```

Rust reads only `fork/rust/fork_config.json`; OCaml reads `new/ocaml-base.json`
and its own `fork/ocaml/fork_validation/legacy/daemon.json` with a copy of its
tarballs. Rust loads the new fork's blockchain circuits and
`chain-id-digests.txt` and `fork-bundle.json` from `--blobs`, checks the exact
configured fork and computed identity plus every file hash, derives its
verification key from the circuits, and refuses a missing or mismatched bundle.
The web node refuses non-release forks. Early diagnostic exports require
`--allow-before-stop` (HTTP: `allow_before_stop=true`) for every selector.

## Evidence

`chain_summary.py --phase old|new` lists every block either node had in its best
chain (creator, slot, commands, coinbase, when each node first had it) and
whether the final tips agree; `audit.py` (through `enter.sh`) records every
account's balance, nonce and delegate on both nodes.

## Stop

```bash
python3 tools/fork-rehearsal/stop.py --dir "$RUN"
```

It signals only recorded PIDs and their descendants (including the namespace
keeper) and prints the remaining list, which must be empty. Delete `keys/` once
the public evidence is collected.
