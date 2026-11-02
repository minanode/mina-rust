# Differential transaction prover gate

This offline gate gives the same Mesa transaction-work specification and SOK
message to Rust's `prove_work` and OCaml's production
`Snark_worker.Impl.perform_single`. It checks each resulting proof with both
implementations, including the expected statement and SOK digest. Proof bytes
need not match.

The corpus has 108 distinct job statements from mainnet and devnet: 71 ordinary
base jobs, 22 merges and 15 zkApp jobs. Base coverage includes payments,
delegations, coinbase, fee transfers, failed payments and failed delegations.
The zkApps include signatures, proofs, single and multiple account updates,
nested calls, custom gates, and proofs from circuits with the largest step
domain, 2^16 rows. One of those is a real failed zkApp. **Four zkApp jobs are
constructed**, one per custom gate (see
[Custom-gate zkApp jobs](#custom-gate-zkapp-jobs)): their statements are not the
chain's. The corpus does not meet the earlier proposed quota of 60 zkApp jobs.
Keep that gap visible when interpreting a passing gate.

## Build the adapters

Build only the ledger example, with embedded circuits. Set `CORPUS_RUN` to your
own evidence directory. On the shared development machine, keep at least 55 GB
free before downloads, builds and proving; remove your build target when done.

```sh
df -h .
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 \
  tools/regression/lock.sh build -- \
  cargo build --locked --release -p mina-tree --features embedded-circuits \
  --example prover_div_corpus
```

The reference adapter builds against Mina OCaml commit
`73134bfd3a43c3f4d8ee564b3d04a0c6ed2e0b3c` (4.0.3). Start with that checkout's
working OCaml 4.14.2 toolchain and pinned `opam.export`. Copy the `ocaml/`
directory beside this README to `src/app/corpus_worker/` in that checkout, then
build `src/app/corpus_worker/corpus_worker.exe` with
`dune build -j 8 --profile dev`. Wrap the entire compile in the same build lock
and environment above. No protocol or verification code needs changing.

Set `OCAML_WORKER` to that executable and configure its runtime library search
path if the compiler used locally installed dependencies. Use a private
`TMPDIR`. The gate creates one inside its result directory and sets
`MINA_PROFILE` separately for each network. It always requests full proofs.

Keep the worker and its runtime libraries in a user-level tools directory,
outside disposable worktrees. On the shared development machine, the installed
bundle is named `mina-4.0.3-prover-div`. Set `PROVER_DIV_TOOLS` to that bundle's
directory and use its wrapper:

```sh
export OCAML_WORKER="$PROVER_DIV_TOOLS/bin/corpus_worker"
```

The bundle's README records the source commits, build commands, executable
SHA-256 and release-subset result. Preserve the entire bundle: the wrapper finds
its libraries relative to itself. Check runtime dependencies with
`LD_LIBRARY_PATH="$PROVER_DIV_TOOLS/lib" ldd "$PROVER_DIV_TOOLS/libexec/corpus_worker.exe"`
before deleting the build tree; no dependency may resolve into a worktree.

The installed 4.0.2 `internal run-snark-worker-single` command cannot replace
this adapter: it selects a random SOK public key and discards its output proof.
The uptime delegation verifier checks a different envelope and is also not a
transaction-worker adapter.

## Obtain the pinned inputs

The corpus manifest and fetcher are private release tooling and are omitted from
the public source export. Obtain them from the release coordinator. The fetcher
accepts a locally supplied archive or downloads the pinned release asset with
authenticated `gh release download`. It verifies the archive's size and SHA-256
before extraction, rejects unexpected paths and links, then checks every job's
SHA-256. It never reads a wallet or node store.

```sh
python3 -B "$CORPUS_FETCHER" --manifest "$CORPUS_MANIFEST" \
  --archive "$CORPUS_ARCHIVE" --output "$CORPUS_RUN/inputs"
```

Omit `--archive` to download. The manifest records complete statements, binary
input hashes, canonical JSON hashes of each witness component, source block
provenance and original bundle positions. Deduplication uses network and the
complete statement's binprot hash, not filenames or bundle counts. Merge jobs
retain their original child proofs; newly generated children are not substituted
in this corpus.

## Run the release subset

Building the adapters and obtaining the inputs are prerequisites. The 16-job
smoke subset covers both networks, all ordinary base kinds, failed ordinary
transactions, merges and proof-authorized zkApps. Among them are the devnet
zkApp job with a 2^16-row circuit that panicked the Rust worker, a real failed
zkApp from such a circuit, and one constructed job per custom gate. Runtime is
measured in `results.json`; a release manager should budget 30 minutes on a
comparable machine, and retain the measured runtime with the release evidence.
The 2026-10-30 run took 8 minutes 37 seconds including process startup and
shutdown; all 64 positive cells accepted and all 64 negative-control cells
rejected.

```sh
ulimit -s 65532
python3 -B tools/prover-div-corpus/run.py \
  --manifest "$CORPUS_MANIFEST" --corpus "$CORPUS_RUN/inputs" \
  --rust target/release/examples/prover_div_corpus --ocaml "$OCAML_WORKER" \
  --output "$CORPUS_RUN/smoke" --subset smoke --negative-controls
```

Use a new output directory for each run. `--subset all` runs every job;
`--case ID` selects an individual reproducer. Job and requested IDs must be
unique, every requested ID must exist, and the full corpus must match
`target_jobs`. The gate reconciles every expected case and verification cell
before passing. Neither mode starts a node, signs transactions or publishes
SNARK work. The SOK uses a fresh public address; no private signing key is
retained or required.

By default, each implementation keeps one worker process per network and runs
jobs sequentially. The report records worker thread count, first-process use,
proof time, wall time and process RSS sampled every 20 ms during each operation,
plus the process lifetime high-water mark. Sampled RSS can miss shorter peaks.
Verification may initialize OCaml keys before its first proof; compare warm
proofs separately from initial loading. `--process-mode cold` instead starts a
fresh process for every operation and records GNU `time` maximum RSS. Builds are
excluded from gate time. Timeouts and missing or inconsistent receipts are
**unavailable**, never accepted or cryptographically rejected. Warm workers are
closed and reaped before the final verdict. Their OS exit statuses are recorded
in `worker_exits`; any nonzero exit, signal or forced termination fails the
gate, even after a successful final operation receipt.

An acceptance/rejection disagreement stops immediately and preserves the input
identifier, proof hash, logs and single-job manifest entry. Re-run that case
before changing any verification code. Any unavailable or rejected positive case
fails the gate. The negative controls change the SOK fee and the network; both
verifiers must explicitly reject them. A passing run establishes the four
verification cells for these inputs, not independent circuits, complete zkApp
coverage or network work publication.

## Capture and harness checks

`capture.py` reads public job data and protocol states from the loopback API of
an observer you own. It requires a synced node, performs only GET requests and
does not enable the worker. The Rust normalizer checks protocol-state hashes
before attaching bodies to witnesses. Retain captures outside git and stop the
observer when finished. The normalizer also accepts the existing repository
base, merge and uptime fixtures. Capture is separate from the release gate.

```sh
python3 -B "$CORPUS_HARNESS_TEST"
```

The coordinator supplies this harness test alongside the private fetcher. It
exercises receipt handling, archive integrity, extraction boundaries and the
disk floor. These are harness tests, not cryptographic coverage.

## Custom-gate zkApp jobs

Every devnet zkApp proof that uses foreign field multiplication, xor, rot or the
lookup gate comes from a circuit with a 2^16-row step domain. That holds for
blocks 552017 to 581982. So a real job tests the largest side-loaded domain
together with the gates. `zkapp-gates/gen.mjs` makes small jobs that test each
gate on its own. It deploys one o1js 3.1.0 contract with a method per gate, on a
`LocalBlockchain` with real proofs and testnet signatures. It then proves one
command per method. `zkapp_gate_jobs` applies the commands to a new ledger, as
the only transaction of a block on a devnet parent, and writes one base job
each. The statements are self-consistent, not the chain's. Install o1js in its
own directory and pass its `dist/node/index.js`.

```sh
node tools/prover-div-corpus/zkapp-gates/gen.mjs \
  "$O1JS/node_modules/o1js/dist/node/index.js" "$GATES"
cargo run --release -p mina-cli --no-default-features \
  --example zkapp_gate_jobs -- --network devnet \
  --parent-block "$PARENT_BLOCK_JSON" --inputs "$GATES/zkapps.json" \
  --out "$GATES/jobs"
target/release/examples/prover_div_corpus devnet normalize \
  "$GATES/jobs/xor_base_job.bin" "$GATES/xor" base
```

The real jobs come from replaying devnet from a staged ledger dump with
`tools/block-replay/examples/mesa_zkapp_job_fixtures.rs`. Pass
`--want NAME=SHAPE@HEIGHT` for each job. The tool also writes OCaml's on-chain
proof of a job when a later block buys it.

## 4.0.4 source audit

The `snark_worker/prod.ml` and `transaction_snark.ml` implementations are
byte-identical in 4.0.3 and 4.0.4, so the adapter's source reference now names
4.0.4. Build recipes, the installed bundle, `assemble.py` and corpus manifests
retain 4.0.3 provenance: no new worker was built or corpus regenerated here.
Pickles and build dependencies changed upstream; a 4.0.4 bundle needs separate
qualification. The pin audit tracks these qualification follow-ups.
