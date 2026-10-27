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
