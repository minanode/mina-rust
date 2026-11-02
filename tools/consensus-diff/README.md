# Compiled OCaml consensus differential harness

This offline harness compares Rust production functions with compiled OCaml
4.0.4 at `c9e2eac5344da621828c1e96c074b8bdce665b51`. It starts no node and
accepts no wallet or producer key. VRF inputs use public synthetic scalar
identifiers. Mainnet and devnet run in separate processes with their network
profiles.

## Build

Build the Rust adapters from the repository root:

```sh
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 \
  CARGO_INCREMENTAL=0 tools/regression/lock.sh build -- \
  cargo build --locked -p mina-node --example consensus_diff \
  -p mina-node-common --example producer_trace
```

Prepare a private copy of the pinned OCaml source and build environment, then:

```sh
python3 -B tools/consensus-diff/prepare-ocaml.py "$OCAML_SOURCE"
```

The preparation script verifies exact source hashes before adding the JSON hook
to `proof_of_stake.ml` and one declaration to its interface. It does not replace
any protocol function. Build `src/app/consensus_diff/consensus_diff.exe` with
the pinned environment's `dune build -j 8 --profile dev`, through the same
build-lock prefix. Keep a private `TMPDIR` and at least 55 GB free.

`install.py` installs the executable, relative runtime wrapper, library bundle,
source hook and reproducible README. Its `SHA256SUMS` includes the executable
and the build recipe. The runtime bundle may use the retained 4.0.4 tools
libraries (with the documented 4.0.3 dependency environment). Verify dependency
resolution with `ldd` and remove private build trees after validation.

## Run

`epoch_start_deferred` keeps its name from BP-MN-3, where Rust waited while
OCaml selected next data: 25 generated cold starts and one pinned reproduction
per profile. Since BP-MN-3A the producer evaluates the clock's epoch at once
with the tip's next epoch data, and these cases must agree like any other. No
mismatch is waived.

The epoch-selection adapter serializes the actual selected context's seed,
ledger and total currency. The runner projects the same fields from OCaml.
`epoch_precompute` exercises Next with the existing finalized/root gate while
the clock remains in the tip's epoch. Rust and runner mutation controls cover
all three payload fields without changing the context variant.

Set `OCAML_ORACLE` to the installed wrapper and `RESULTS` to a new directory:

```sh
python3 -B tools/consensus-diff/run.py \
  --rust target/debug/examples/consensus_diff \
  --rust-trace target/debug/examples/producer_trace \
  --ocaml "$OCAML_ORACLE" --output "$RESULTS"
```

`--rust-trace` adds the `epoch_traces` area (`traces.py`): multi-step traces
through the block producer's own reducers and enabling conditions, with the VRF
service worker and its disk cache answering requests in a private temporary
directory. Steps move the clock and replace the best tip; a step may make the
staking ledger unavailable or restart the node with its cache. Requests still in
flight when a step replaces the tip are answered at the start of the next step.
Traces cover cold starts after a boundary, restarts with persisted results, an
unavailable then restarted ledger, a delayed ledger, reorgs across the seed lock
point (early and precomputed), a reorg across the epoch boundary, a skipped
epoch, a same-slot sibling after an empty epoch (the block is made on the tip's
parent, two epochs back) and a completed epoch with changed inputs. Each step
compares the epoch data the producer is bound to for the clock's epoch (and the
next one where the step expects precomputation), every win in the step's window
with its VRF output, whether a block is started for the clock's slot, and
invariant violations. Compiled OCaml supplies the epoch data
(`select_epoch_data`), child epoch data across a boundary
(`Epoch_data.update_pair`) and every VRF output. Two expectations are producer
policy, stated per step: a missing ledger is waited for, and precomputation
happens where the step says so.

Use `--corpus "$SAVED_CORPUS"` to include recorded Mesa chains. That directory
must contain `replay/chain.txt` with its `blocks/` directory and
`replay-devnet/chain.txt` with adjacent block files. Only `protocol_state` is
parsed; unrelated transaction fields do not enter these comparisons. Parent
links are checked before comparing both selection orientations. Use
`--forks "$SAVED_FORKS"` for Mesa sibling blocks named with the network, height
and state hash. Pre-Mesa transaction versions are excluded.

When the oracle is a wrapper, pass `--ocaml-executable "$OCAML_EXECUTABLE"` to
record the actual executable digest as well as the wrapper digest.
`regressions.json` retains minimal inputs and compiled OCaml outputs for the two
original divergences; the runner verifies those outputs against the live oracle
on every full run.

The runner stores each compared input and both outputs, counts by area and
network, binary hashes, Rust build metadata, elapsed time and worker exits.
`divergences.json` retains complete reproductions. An adapter error or any
mismatch fails the run. JSON types, numeric strings, protocol hashes, VRF bytes
and Boolean results remain exact; object key order is ignored.

Generated states exercise individual functions and are not asserted to have
valid block proofs. Threshold edges are calibrated with compiled OCaml, then
Rust is checked immediately below, at and above each cutoff. Existing Rust
transcription tests and producer reducer regressions remain separate evidence.
See the BP-MN-3 research note for measured coverage and unresolved limits.

## 4.0.4 qualification

The full-file source guard requires the peeled 4.0.4 commit. Its bootstrap
status plumbing and extracted protocol-state helpers do not alter the consensus
functions called by the hook. `account_defaults` additionally exposes upstream
empty-account JSON and its digest for the fixed default-permissions regression.

`qualification.json` and `regressions.json` preserve their original 4.0.3
provenance; they are historical evidence, not relabeled 4.0.4 runs. The 4.0.4
qualification receipt is `qualification-4.0.4.json`; it records the 52
epoch-start differences open at the time. BP-MN-3A's receipt,
`qualification-bp-mn-3a.json`, records the same corpus with those cases
agreeing, and the producer traces.
