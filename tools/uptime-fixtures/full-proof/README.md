# Full-proof OCaml uptime captures

These are unmodified HTTP requests emitted by official OCaml Mina **4.0.3** on
public devnet, with full blockchain and transaction proofs. All reporting URLs
were loopback. The fresh unfunded submitter's private key is **not** included.
The earlier `../fixtures/` private-chain captures contain dummy proofs.

## Captured cases

| Height | Selected Transition | Created at (UTC, 2026-10-28) | Proving seconds    |
| ------ | ------------------- | ---------------------------- | ------------------ |
| 581348 | Coinbase            | 19:33:44                     | 44.009031534194946 |
| 581351 | Signed payment      | 19:40:43                     | 13.21396255493164  |
| 581352 | Signed payment      | 19:48:13                     | 13.72036099433899  |

Each directory contains the exact `uptime-1.json`, compact signed
`uptime-data.json`, HTTP record, provenance manifest, OCaml-decoded block/proof
fields, selected job including its public witness, ordered candidate summary,
two uptime log events, and byte offsets for every field in `wire-layout.json`.
Do not pretty-print the two exact payload files. `selection.json` records the
separately sampled work snapshot time and raw GraphQL hash. Its first matching
Transition's complete statement equals the captured proof statement. It is
evidence consistent with the source selection rule, not instrumentation of the
service's function call.

`selected-job.bin` is the same selected job decoded from JSON and encoded to
Bin_prot by OCaml 4.0.3's `Snark_work_lib.Spec.Single.Stable.Latest`, using
`uptime_inspect job selected-job.json selected-job.bin`. Its SHA-256 and origin
are in the manifest. Rust tests consume this public witness directly, without
constructing their own expected statement or ledger.

The fee is `0.001` MINA (1,000,000 nanomina). Each proof binds
`Sok_message(fee, submitter)` with digest
`b971b06144c49e3bfcf1f93a88c03c3a9d1ccfe4520abee60fae6c9dc3ddf7b4`. `snark_work`
is Base64 of **unversioned** Bin_prot `Proof_data`, ordered as `Ledger_proof`,
float64 `Time.Span` seconds, stable `Currency.Fee`. Each decoded blob re-encoded
byte-for-byte with OCaml. The outer signature covers the BLAKE2b-256 digest of
the exact compact `data` JSON in the testnet domain.

No natural no-suitable-job, selected zkApp, or produced-block report is claimed
by these fixtures. The unfunded non-producer cannot exercise block production.
The verification harness derives absent-work and unrelated-work controls from
real captures; those controls are **not emitted daemon reports**.

## Verification

`verification-results.json` records the independent reference verifier results.
It does not replace rerunning cryptographic verification. `check_full_proof.py`
checks byte integrity, SOK digest, target selection and recorded results;
`signature-check` independently verifies the outer OCaml signatures in Rust.

```sh
python3 -B tools/uptime-fixtures/check_full_proof.py
CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR="$UPTIME_WORK/signature-target" \
  tools/regression/lock.sh build -- cargo run --locked \
  --manifest-path tools/uptime-fixtures/signature-check/Cargo.toml \
  -- tools/uptime-fixtures/full-proof
MINA_PROFILE=devnet TMPDIR="$UPTIME_WORK/verify-tmp" \
  python3 -B tools/uptime-fixtures/verify_full_proof.py \
  --verifier "$UPTIME_SOURCE/_build/default/src/app/delegation_verify/delegation_verify.exe" \
  --config tools/uptime-fixtures/full-proof/devnet-config.json \
  --output "$UPTIME_WORK/verification-new"
```

Create the private `TMPDIR` first. Set `LD_LIBRARY_PATH` to your user-local
native dependency prefix if necessary. Verification initializes devnet genesis
ledgers and keys and can download their public artifacts; it sends no report.
The result directory must not exist. Checks remain enabled; never add
`--no-checks`. Upstream stdin output only **replaces existing result fields**,
so the harness supplies sentinels. `verified: true` is also returned on errors;
acceptance requires `validation_error: null` and populated matching block
fields. The verifier does not check the outer HTTP signature or job selection.

## Obtaining the verifier

No verifier binary was present in the official daemon package or checked package
indexes. It was built at source commit
`73134bfd3a43c3f4d8ee564b3d04a0c6ed2e0b3c`, using OCaml 4.14.2 and that commit's
`opam.export`. `build-provenance.json` records hashes and dependency versions.
All packages were downloaded and extracted user-locally; no system installation
was used. The upstream verifier logic was unchanged.

After initializing the pinned opam repositories and importing `opam.export`,
build inside the upstream checkout:

```sh
ulimit -s 65532
ulimit -n 10240
MINA_PROFILE=devnet CARGO_BUILD_JOBS=8 dune build -j 8 --profile dev \
  src/app/delegation_verify/delegation_verify.exe \
  src/app/uptime_inspect/uptime_inspect.exe
```

On the shared development machine, wrap the entire build in the repository's
`tools/regression/lock.sh build -- ...`. Copy `../ocaml-inspect/` into the
upstream `src/app/uptime_inspect/` directory to build the decoder. The decoder
uses upstream OCaml types and verifies testnet outer signatures, but does not
verify proofs. Use profile `dev`, as in upstream's default Makefile: `release`
failed on an upstream inline-test-dependent polymorphic generator. For a
checkout nested in this Rust workspace, append an empty `[workspace]` table to
upstream `src/lib/crypto/kimchi_bindings/stubs/Cargo.toml` to isolate Cargo's
workspace lookup. That build-only change does not alter verifier or protocol
logic.
