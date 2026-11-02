# Web node next release (draft)

Minaweb and the downloadable devnet web bundle are planned for a separate
release after native 0.20.1. The version and publication date are not frozen. No
web archive belongs in the native 0.20.1 assets or checksum list. Keep the web
download page hidden throughout the native 0.20.1 publication.

## Minaweb application

The minaweb app adds blocks, accounts, transaction-pool views, startup progress
and browser-store reset. Devnet builds add key generation and encrypted storage,
payment submission, a SNARK-worker screen and a block-producer screen. A tab
runs one role at a time. Changing roles restarts the node.

Browser tests exercised key handling, unfunded-payment rejection and worker
proof offers. They did not establish funded-payment inclusion, worker purchases
or staked production for the new app. The public devnet deployment kit includes
relay, TURN and split-origin asset checks. A kit does not establish that the
public service has launched.

## Browser integrity and privacy

Browser prover files must match the canonical OCaml SHA-256 pins in both
same-origin and external-asset builds. The bridge bounds prover initialization
and reports integrity, initialization and timeout errors during role startup.
The downloadable bundle includes all 24 pinned devnet prover files.

The browser connects only to its page's configured seed relay addresses by
default. Peer discovery and signaling exchange are disabled. The relay, static
host and configured STUN, TURN and asset hosts can still see the visitor's
network address. Invalid genesis inputs return explicit browser startup errors.
Native signaling opt-in and native RPC additions remain in native 0.20.1.

## Downloadable web bundle

The separate `tools/release/build-web.py` command produces an unsigned
`mina-web-<version>.tar.gz` with the application, WASM, circuit files,
dependency notices, per-file hashes and a loopback Python server. The server
verifies the unpacked files before serving them and accepts an explicit relay
override. The native `tools/release/build.sh` does not invoke this command.

For the web release, choose its own reviewed commit and workspace version. Set
`WEB_VERSION` to that version, `WEB_DIST` to a new output directory separate
from the native release, and `RELAY` to the approved public relay multiaddr:

```sh
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
  tools/regression/lock.sh build -- \
  python3 -B tools/release/build-web.py "$WEB_VERSION" --output "$WEB_DIST"
(cd "$WEB_DIST" && sha256sum --check SHA256SUMS)
```

The packager requires the requested version to match the workspace version.
Regenerate and commit its licence inventories from that release's exact WASM and
JavaScript build before packaging. Qualify the archive with the separate web
gates in [RELEASING](RELEASING.md#separate-web-release-not-0201), including
clean unpack, integrity checks and fresh browser sync. The owner signs and
publishes a separate checksum set. Never append it to the 0.20.1 native set.

Before making the hidden download page public, give it separately versioned web
release metadata and verified download URLs. Native publication alone must not
unhide it. Browser deployments need the new page/WASM bundle and a qualified
relay; replacing a native binary alone does not update a hosted browser bundle.

## Known limitations and launch gates

- minaweb's TURN credentials load at boot. A tab past their expiry may need a
  reload to reconnect.
- minaweb can round very large displayed balances or stakes across the WASM
  boundary. Signed payment amount and fee strings remain exact.
- Public relay capacity still needs separate qualification. The public
  deployment kit targets devnet.

Funded-payment inclusion, worker purchases, staked production, relay capacity,
DNS/TLS and direct/forced-TURN launch checks still need their own qualification.
This draft is not a launch or publication claim.
