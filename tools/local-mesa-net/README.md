# Private Mesa acceptance experiment

Run OCaml 4.0.2 against a Rust producer using a new private ledger, full proofs,
and the Mesa **mainnet** (default) or **devnet** constraint constants, fork
constants and circuit blobs. Choose `NETWORK=mainnet` or `NETWORK=devnet` below.
`prepare.py` saves the selection in `network.json`; the node and payment helpers
read it and reject a mismatched fork or genesis identity. Older run directories
without that file retain the mainnet default. The scripts require Linux
unprivileged user/network namespaces, `ip`, `nsenter`, Python 3 and the
repository's OCaml binary wrapper. No sudo, container engine or public-network
peer is needed. The namespace has only loopback and no external route. The
default procedure gives each daemon its own shared node lock. An explicit
coordinator allocation for P4.6c permits the fully isolated pair without node
locks, with a 45-minute cap; see the allocated-run procedure below.

Reserve both shared node slots before generating the future timestamp. OCaml
exits if it finds no peer within a minute after its transition router starts;
older producer revisions also have a late-genesis startup limitation (fixed by
P4.6a revision `907856a4`, integrated on this branch). The reservation
supervisor uses the normal shared locks. If it cannot immediately acquire the
second slot, it releases the first and retries later, avoiding deadlock with
another paired experiment. If single-node jobs repeatedly take the first freed
slot, `--partial-wait 120` holds the first slot for at most two minutes while
waiting for the second, then releases it before retrying. The timestamp is still
generated only after both are held. Once both are held, the daemons retain their
locks until they exit.

Build **before** generating the timestamped configuration. From this worktree:

```bash
ROOT=$(git rev-parse --show-toplevel)
TARGET=$ROOT/target
CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=$TARGET "$ROOT/tools/regression/lock.sh" build -- \
  cargo build --release -p mina-cli --bin mina \
  -p mina-block-replay --example local_mesa_identity
```

Copy the two executables to a task-private binary directory so another build
cannot replace them during a run. Use a fresh run directory outside the repo;
`prepare.py` refuses to overwrite one. It generates four encrypted account keys,
two encrypted libp2p identities and a random password under `keys/`, with
private permissions. Never print, copy into the repository, or publish those
files. `daemon.json` contains public accounts only. The producer has 1,000,000
MINA; three other accounts have 1,000 each. All self-delegate. The protocol's
public genesis winner is account zero with 0.000001 MINA.

```bash
RUN=/tmp/my-private-mesa-run
NETWORK=mainnet
MINA=/absolute/path/to/private/bin/mina
IDENTITY=/absolute/path/to/private/bin/local_mesa_identity
BLOBS=/tmp/p4/gen2/circuit-blobs
# For devnet: NETWORK=devnet; BLOBS=/tmp/p4.8/gen/circuit-blobs
LEASE=/tmp/my-private-mesa-slots
python3 tools/local-mesa-net/reserve.py --lease "$LEASE" --dir "$RUN" \
  --mina "$MINA" --blobs "$BLOBS" &
until test -f "$LEASE/ocaml.held" && test -f "$LEASE/rust.held"; do sleep 1; done
python3 tools/local-mesa-net/prepare.py --dir "$RUN" --mina "$MINA" \
  --ocaml scripts/ocaml-mina --network "$NETWORK" --genesis-delay 420
cp "$LEASE/ocaml.held" "$RUN/ocaml.pid"
cp "$LEASE/rust.held" "$RUN/rust.pid"
python3 tools/local-mesa-net/namespace.py --dir "$RUN" > "$RUN/logs/namespace.log" 2>&1 &
```

Wait for `namespace.json`, then compute both identities. OCaml's second command
also generates a real genesis proof. All OCaml commands use a private `TMPDIR`.
`enter.sh` sets `TAR_OPTIONS=--no-same-owner` because a one-UID user namespace
cannot restore host archive ownership. This affects only its command and
children.

```bash
"$IDENTITY" --network "$NETWORK" --config "$RUN/daemon.json" --out "$RUN/rust-identity.json"
bash tools/local-mesa-net/enter.sh "$RUN" env TMPDIR="$RUN/tmp" MINA_PROFILE="$NETWORK" \
  scripts/ocaml-mina internal chain-id --config-file "$RUN/daemon.json" \
  --config-directory "$RUN/identity-conf" --genesis-ledger-dir "$RUN/genesis" \
  > "$RUN/logs/ocaml-chain-id.log" 2>&1
bash tools/local-mesa-net/enter.sh "$RUN" env TMPDIR="$RUN/tmp" MINA_PROFILE="$NETWORK" \
  scripts/ocaml-mina internal test-genesis-block-generation \
  --config-file "$RUN/daemon.json" --config-directory "$RUN/proof-conf" \
  --genesis-ledger-dir "$RUN/genesis" > "$RUN/logs/ocaml-genesis-proof.log" 2>&1
python3 tools/local-mesa-net/match_identity.py --dir "$RUN"
```

The comparison checks every genesis protocol-state field (normalizing only hex
letter case) and the independently computed chain IDs. The Rust helper rejects
the public mainnet and devnet IDs. Keep the selected profile's `proof.fork`:
changing it changes the blockchain circuit/key. OCaml's `MINA_PROFILE` selects
its compiled profile; the runtime fork comes from the matching
`tools/mesa-circuits/mesa-{mainnet,devnet}-fork.json`. Devnet uses `4.0.2devnet`
blobs and mainnet uses `4.0.2mainnet` under `BLOBS`. Genesis and both epoch
ledgers are explicit and identical in the runtime config, which both nodes read
directly.

Start OCaml first. Defaults use agent ports 3660/8860 for Rust and
3670/8870/8871 for OCaml; override `--rust-slot` and `--ocaml-slot` together if
needed. These ports are also private to the network namespace. The helper
requires the lock descriptor and refuses to launch in the host namespace. Rust
uses `--max-peers 2` with just one explicit peer: its stable-peer limit is
floor(0.8 times the maximum), so a maximum of 1 periodically evicts the only
peer. Discovery is disabled and the namespace cannot reach any external peer.
The Rust launch explicitly enables transaction submission with
`--enable-tx-submission` for the throwaway payment; P6 disables it by default.

```bash
touch "$LEASE/ocaml.start"
```

Once OCaml's GraphQL server is ready, confirm the daemon's own genesis state
hash and chain ID. The producer refuses to start until this succeeds; a changed
config invalidates the saved match by SHA-256.

```bash
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/local-mesa-net/match_identity.py \
  --dir "$RUN" --ocaml-port 3670
touch "$LEASE/rust.start"
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/local-mesa-net/sample.py \
  --dir "$RUN" --seconds 5400 &
```

`samples.jsonl` records Rust production timings/status and peers alongside
OCaml's GraphQL best chain every ten seconds. OCaml's detailed JSON log is
`ocaml-netns/mina.log`. Rust archives blocks under `blocks/`. Count acceptance
by matching each Rust-produced hash to OCaml's best chain and validation log,
never from receipt or sync status alone.
`Block proof verified in prover service` records verification against the
selected network's key; the P15 state machine also checks the proof before
injecting the block. No verification bypass is enabled.

`report.py` preserves the first receipt, every repeated receipt, and all OCaml
rejections per block hash. An accepted block may later be announced again and
rejected as too old; report both events. The report also records completed SNARK
work, coinbase, and fees. Empty completed-work counts do not mean dummy block
proofs: an initially empty scan state can admit coinbases and a payment before
it needs to emit a transaction ledger proof.

All API calls and payment submission must run through `enter.sh`. After the
first block, submit a 1 MINA payment with a 0.1 MINA fee from `keys/sender` to
`keys/receiver.pub`:

```bash
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/local-mesa-net/audit.py \
  --dir "$RUN" --label before-payment
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/local-mesa-net/payment.py \
  --dir "$RUN" --mina "$MINA"
python3 tools/local-mesa-net/report.py --dir "$RUN"
```

The payment helper loads the generated password into the wallet child
environment without printing it or placing it on the command line. It captures
the existing CLI's signed GraphQL mutation on loopback port 3661 and submits the
same payment to the existing `/send-payment` REST endpoint. This retains P4.6c's
submission path, originally a workaround for GraphQL's nonce-zero rejection
(fixed by P6), without changing node validation. The capture response
intentionally makes the signing CLI exit 1; check `payment-response.json`, the
transaction pool, and the accepted block for the actual submission result. No
real account key is used.

After the payment appears in OCaml's accepted chain, run `audit.py` again with
`--label after-payment`. It saves both nodes' balances and nonces for all four
generated accounts, plus OCaml's trust status and bans. Compare the saved
responses and check for GraphQL or audit errors before claiming ledger parity.

Stop the experiment even if a check fails:

```bash
python3 tools/local-mesa-net/stop.py --dir "$RUN"
python3 tools/local-mesa-net/stop.py --dir "$LEASE"
```

The sampler tracks descendants and Linux process start times. Cleanup signals
only those recorded PIDs, guards against PID reuse, and includes the namespace
keeper. `--role rust`, `--role ocaml`, or `--role sample` can stop one
component. The lease PID files also cover a process still waiting for a node
slot. If setup fails before the run directory exists, stop the lease directory
alone. Confirm the reported remaining-process list is empty and the locks held
by this run are released. Research results belong in private implementation
notes and the Notion task; private keys and raw witness dumps stay in the run
directory. Delete generated `keys/` after collecting the public evidence and
stopping all processes.

## Explicitly allocated isolated run

On 2026-10-21 the coordinator authorized P4.6c's and P4.6c-devnet's
route-isolated private pair without node slots: those slots limit public-mainnet
connections from this machine's IP. This is an explicit allocation for this
experiment, not a change to the shared lock policy. Builds still use
`lock.sh build`. Release any partial reservations with `stop.py` before using
this procedure.

Skip `reserve.py` and the `.held`/`.start` steps. Prepare a fresh run directory,
start its namespace, compute both identities and complete `match_identity.py` as
above. Then launch OCaml:

```bash
python3 tools/local-mesa-net/run_private.py ocaml --dir "$RUN" --mina "$MINA" \
  --blobs "$BLOBS" --private-isolated-allocation > "$RUN/logs/ocaml-netns.log" 2>&1 &
```

Wait for GraphQL, run the live `match_identity.py --ocaml-port 3670` check, then
launch Rust and the sampler:

```bash
python3 tools/local-mesa-net/run_private.py rust --dir "$RUN" --mina "$MINA" \
  --blobs "$BLOBS" --private-isolated-allocation > "$RUN/logs/rust-netns.log" 2>&1 &
bash tools/local-mesa-net/enter.sh "$RUN" python3 tools/local-mesa-net/sample.py \
  --dir "$RUN" --seconds 2700 &
```

The launchers share a deadline 2700 seconds after the first launch and use
`timeout --foreground` to signal their own daemon PID if it expires. Each node
rechecks that only loopback exists and no default or external route exists, and
writes its isolation preflight. The normal config-hash, peer-list, full-proof
and live-genesis gates still apply. Record `namespace.json`, the preflight
files, `identity-match.json`, and the allocation in the research note. Stop
before the deadline when the measurements are complete; `stop.py` tracks the
launcher PIDs, daemon PIDs, sampler, namespace and descendants. Run it even if a
timeout or other error occurs, and require an empty remaining-process list.
