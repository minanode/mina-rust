# Run the web node yourself

This is an experimental Mina devnet node. The JavaScript, WASM, fonts, genesis
configuration and canonical prover files are served from your own machine.
Prover files are included: their compressed size is small enough to avoid a
separate download and asset host. Verifier caches are omitted so local
build-machine caches cannot change the archive; the node constructs its verifier
indexes at startup.

## Check the download

Before unpacking, authenticate the archive using `SHA256SUMS` and
`SHA256SUMS.sig` from the same release. Follow
<https://minanode.org/docs/install/#check>, including checking the signer
fingerprint through a separate trusted channel. A matching hash alone does not
authenticate the publisher.

## Start

Install Python 3.11 or newer, unpack the archive, and run from its directory:

```sh
python3 -B serve.py
```

Open <http://127.0.0.1:8080/> in a current desktop Chrome or Chromium browser.
The server verifies every bundle file against `manifest.json` before listening.
To check without starting it, use `python3 -B serve.py --verify-only`. Use
`--port 8081` if the default port is occupied. Press Ctrl-C to stop.

Loopback counts as a secure context. The server sends COOP and COEP headers for
WASM threads and binds only to `127.0.0.1`. Opening `index.html` directly, or
using an ordinary static server without these headers, will not work. Keep the
unpacked files unchanged and use a fresh directory for each release.

The page verifies the WASM hash before running it. The server's manifest check
also covers the circuit files. This moves trust from a site's changing
JavaScript to the release you verified; it does not remove trust in the release
publisher, your browser or your computer. Use only devnet keys.

## Choose a relay

By default the node dials the release's relay, recorded in `manifest.json`. The
browser uses exactly that configured relay, with discovery disabled. The relay
and STUN servers can still see your network address. The archive includes no
TURN credentials; restricted networks may block direct WebRTC.

To use your own native node on the same computer, start a separate terminal in
the bundle directory. Keep the native data outside it so the bundle's file list
stays unchanged:

```sh
mina node --network devnet --work-dir ../mina-relay-data \
  --http-addr 127.0.0.1 --port 3000 --libp2p-port 8302 \
  --webrtc-signal-addr 127.0.0.1 --webrtc-signal-port 8303
```

Use a native release with the WebRTC signaling listener (0.20.1 or later). Wait
for it to sync and read its public peer ID from the `P2P identity` log line.
Never share the key file. Replace `PEER_ID` below with that public ID:

```sh
python3 -B serve.py \
  --relay /ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/PEER_ID
```

`--relay` replaces the packaged relay without rebuilding or modifying files. The
page reads the local server's `local-relay.json` at each start; no old seed is
retained. Restart the server and reload the page after changing the relay. Do
not enable `--webrtc-signaling-relay`: it enables peer introduction, which this
public-demo model deliberately leaves disabled.

For a remote native relay, provide its reachable signaling multiaddr. Bind its
signaling listener with `--webrtc-signal-addr`, keep the HTTP operator port
private, and allow the WebRTC UDP range selected with `--webrtc-udp-ports`. A
relay behind NAT may need `--webrtc-nat-ip` and port forwarding. Signaling over
HTTPS does not by itself make the UDP connection reachable.

## Relay address format

Packaging and `--relay` accept a WebRTC signaling multiaddr with `ip4`, `ip6`,
`dns`, `dns4` or `dns6`, a TCP port from 1 to 65535, `webrtc/http` or
`webrtc/https`, and an Ed25519 libp2p peer ID. IP families, DNS labels and the
peer ID encoding are validated. Plain URLs, WebSocket addresses and extra
multiaddr components are rejected with an error before serving or compiling.

## Rebuilding

Build from a clean checkout with the pinned Rust toolchain, wasm-bindgen and
Node versions. `tools/release/build-web.py` uses the commit timestamp as
`SOURCE_DATE_EPOCH`, fixes the archive ordering, modes and ownership, and omits
the gzip filename and timestamp. It fixes genesis, features and frontend
options; the only deployment input is `RELAY`. Local verifier caches and
unrecorded frontend overrides are ignored. Reproducibility compares the same
commit, relay and toolchain; compare the SHA-256 from two independent output
directories. A new source commit intentionally produces a different archive.

The archive builder builds a private wasm-bindgen 0.2.106 CLI from
checksum-pinned crates and their locked dependencies. A small patch orders cast
adapters by their original function IDs, removing upstream hash-map iteration
randomness. It does not change the installed CLI or Cargo registry. The two
crate downloads are needed only when they are absent from the Cargo cache.
