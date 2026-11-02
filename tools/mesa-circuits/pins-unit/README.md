# Circuit pin policy tests

This small standalone crate imports the production `circuit_pins.rs` directly.
It runs the five policy tests and a sixth test that decompresses canonical OCaml
gates from both networks and checks the actual bytes against their committed
manifests. It does not duplicate the policy implementation or build the full
ledger test suite.

Run from the repository root, with the pinned Rust toolchain, Node 24,
`wasm32-unknown-unknown`, and `wasm-bindgen-test-runner` 0.2.106 installed:

```bash
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
  tools/regression/lock.sh build -- tools/mesa-circuits/pins-unit/run.sh both
```

Use `native` or `wasm` to run one target. Cargo dependencies and the lockfile
are pinned. The WASM tests run in Node; the script clears the application's
shared memory flags because this focused policy test does not use the node's
threads. Build outputs go under the repository's `target/pins-unit` by default.
