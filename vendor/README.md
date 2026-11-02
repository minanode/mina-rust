# Vendored crates

## rustrtc

Kept in the tree to carry fixes of our own. It is wired in with
`[patch.crates-io]` in the root `Cargo.toml` and excluded from the workspace,
and `cargo fmt`, clippy, taplo, prettier and the whitespace checks skip
`vendor/rustrtc`, so it stays as published apart from our commits.

- Upstream: https://github.com/restsend/rustrtc, MIT (`rustrtc/LICENSE`).
- Version: 0.4.9 from crates.io, the `.crate` with SHA-256
  `783a2080f931fb54e90f1100ee621c8df7609b8199284c41479310ab49ab098d`, built from
  upstream commit `7444cc4b128911b073b7fdeaea1c23c6ad772baf`
  (`rustrtc/.cargo_vcs_info.json`).
- Kept: `src/`, the published `Cargo.toml` (and `Cargo.toml.orig`), its
  `Cargo.lock`, `LICENSE`, `README.md`. Left out: the package's `interop/`,
  `scripts/`, `tools/` and `interop-test.sh`, which the library doesn't use.
- Its `Cargo.toml` accepts `futures` 0.3.31 and `rcgen` 0.14.0 (published:
  0.3.32 and 0.14.8), so adding it to `Cargo.lock` changes no version the
  workspace already uses (`futures` 0.3.31, `ring` 0.17.8, `cc` 1.2.5); the lock
  has `rcgen` 0.14.3. Its own `Cargo.lock` is as published.
- UPnP is a feature of its own, `upnp` (on by default, as upstream), which the
  workspace leaves off (`default-features = false, features = ["std"]`): no
  `igd` or `attohttpc` in the build.
- Our changes are separate commits on top of the published sources, each with a
  test; `git log -- vendor/rustrtc` lists them. Upstream text for them is
  drafted outside the tree.
- Flow control publishes the remote application section before ICE starts, so a
  fast loopback connection always starts SCTP. The sender also reserves 256
  bytes of peer memory per in-flight DATA chunk, separately from network
  congestion accounting. Regression fixtures cover startup ordering and peer
  receive-window accounting.
- Tests:
  `CARGO_TARGET_DIR=<dir> cargo test --manifest-path vendor/rustrtc/Cargo.toml --lib <filter>`,
  which uses the crate's own lock file and dev-dependencies.
- Used by the `p2p-webrtc-rustrtc` backend of `crates/p2p`
  (`src/service_impl/webrtc/webrtc_rustrtc.rs`).

- Raw DTLS ingress, decrypted application records, and SCTP output share the
  bounded `transports::packet_queue` implementation on native and embedded
  targets. Admission checks item, byte and process budgets without waiting for
  the consumer. Local overflow fails the owning DTLS association; aggregate
  pressure instead sheds the association with the largest combined occupancy
  across its three queues. It reclaims the victim's buffers before admitting a
  small newcomer and publishes failure outside the pool lock. Metrics count
  aggregate sheds separately from local admission failures. Payload copies
  prevent small `Bytes` slices from retaining unaccounted backing allocations.
  DTLS now returns this bounded receiver to SCTP; affected test fixtures use the
  same API. SCTP drains at most 64 packets before checking cancellation. The
  peer monitor treats transport-loop completion as terminal even if the
  aggregate-shedding callback has not published DTLS failure yet, preserving
  existing Closed/Failed states. Limits are 4,096 items and 8 MiB per queue,
  65,535 bytes per packet, 24 MiB per association, and 64 MiB across queues.
  Ten-browser bootstrap measurements peaked at 217 items and 203,216 bytes per
  queue (339,488 bytes aggregate), leaving over 18 times the observed packet
  burst and 41 times the per-queue byte burst. Accounting includes copied
  payloads plus a 64-byte allowance per item; it does not bound total process
  RSS.

## wasm_thread

The web node's threads. Kept in the tree, as rustrtc is, so that the web node
page runs under a Content-Security-Policy without `'unsafe-eval'`
(`tools/regression/webnode/csp.py`). Wired in and skipped by the formatters and
checks the same way.

- Upstream: https://github.com/chemicstry/wasm_thread, MIT or Apache-2.0
  (`wasm_thread/LICENSE-MIT`, `wasm_thread/LICENSE-APACHE`).
- Version: 0.3.3 from crates.io, the `.crate` with SHA-256
  `b7516db7f32decdadb1c3b8deb1b7d78b9df7606c5cc2f6241737c2ab3a0258e`, built from
  upstream commit `7ec48686bb1a0d9bd42cf16e46622746e4d12ab3`
  (`wasm_thread/.cargo_vcs_info.json`).
- Kept: `src/`, the published `Cargo.toml` (and `Cargo.toml.orig`), the
  licences, `README.md`. Left out: the package's examples, tests, CI and build
  scripts, which the library doesn't use.
- Our change, one commit on top of the published sources: no `eval`. The global
  scope comes from `js_sys::global()` instead of `eval("self")`; the glue's URL,
  which each worker imports, from its `import.meta.url` instead of a stack trace
  of an evaluated script; and the module-worker polyfill, evaluated on every
  spawn, is gone, as every browser with shared WASM memory has module workers.
  Each worker still starts from a `blob:` script, which the policy allows in
  `worker-src`.
- Tested in the browser only: the web node page boots and starts its prover's
  threads under the policy with no violation (`minaweb-check.mjs`,
  `minaweb-roles-check.mjs` with `CSP_REPORTS`).
