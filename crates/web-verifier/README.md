# mina-web-verifier

Verify Mina blockchain SNARK proofs and pick the best tip, natively or in the
browser. A block whose proof verifies attests, by Pickles recursion, every
ancestor back to genesis, so a client can check the chain against data from an
untrusted source. Competing blocks can all be valid, so the verifier also runs
the node's own fork choice (`mina_core::consensus::consensus_take`) over the
proven consensus states.

The verification key is data, not code. The OCaml node's
`blockchainVerificationKey` JSON is accepted directly, so the Mesa hard fork
needed new keys (`data/vk/`), not a new verifier. The key ingester and the
precomputed-block decoder are adapted from
[MinaProtocol/mina-verify](https://github.com/MinaProtocol/mina-verify)
(Apache-2.0).

## Inputs

- **Gossip payloads**: blocks exactly as they travel over Mina's libp2p gossip.
  Only the header is decoded, which keeps decoding independent of body types
  that hard forks change. The [relay](../web-relay) forwards these to browsers.
- **Precomputed blocks**: the JSON that nodes publish, for example to the public
  `mina_network_block_data` bucket.

## Native

```sh
cargo test --release -p mina-web-verifier
```

The tests verify real Mesa devnet and mainnet blocks, including one captured
from live mainnet gossip, against the live keys. They check that tampered,
truncated and cross-network blocks are rejected, and that fork choice picks the
block mainnet actually built on among three competing blocks at height 563933.
They also verify the first Mesa block on each network (the genesis's child), and
check that a block of the old Berkeley chain from right after the fork point is
rejected: it carries a Berkeley proof.

To check precomputed blocks by hand, the `verify_blocks` example prints, per
block, the height, the protocol state hash it computes, the protocol version and
genesis state hash the block carries, and whether its proof verifies:

```sh
cargo run --release -p mina-web-verifier --example verify_blocks -- \
  data/vk/mainnet-mesa-blockchain-vk.json mainnet-548148-*.json
```

## Browser demo

`www/` is a static page with two sources:

- **Live gossip**: connect to a relay's WebSocket and verify each block as the
  network gossips it.
- **Block archive**: find the newest heights in the public bucket and verify
  every block at them, competing ones included.

It shows the best tip, labels each block as best tip, canonical (with depth) or
off the best chain, and flags a tip that falls behind the clock.

```sh
crates/web-verifier/build-wasm.sh            # pinned nightly, wasm-bindgen-cli 0.2.106
cargo run --release -p mina-web-relay        # optional, for the live source
python3 crates/web-verifier/www/serve.py     # http://127.0.0.1:8740/
```

The page must be served cross-origin isolated (COOP `same-origin`, COEP
`require-corp`) for `SharedArrayBuffer` and wasm threads; `serve.py` sends both
headers. The bucket's JSON API answers with CORS headers, and WebSockets are not
subject to COEP, so neither source needs a proxy.

The main thread may not block, so every expensive call runs on a Web Worker and
returns a promise. The rayon pool runs on further workers (`src/wasm.rs`).

## Browser test

`tests/browser/run.mjs` drives the page in headless Chrome and prints timings:

```sh
cd crates/web-verifier/tests/browser && npm install
CHROME=/path/to/chrome-headless-shell node run.mjs http://127.0.0.1:8740/ bucket 5
CHROME=/path/to/chrome-headless-shell node run.mjs http://127.0.0.1:8740/ live 3
```

## Measurements (2026-10-18, 32-core x86_64, Chrome 155 headless)

| Step                                            | Time                               |
| ----------------------------------------------- | ---------------------------------- |
| Load wasm (5.0 MB) and start 31 workers         | about 0.15 s                       |
| Build verifier index and SRS (once per network) | about 4.0 s                        |
| Build proof-checking SRS (once)                 | about 3.4 s                        |
| Verify one block proof                          | about 3.8 s (native: about 0.14 s) |

The SRS setup could be replaced by downloading precomputed SRS and
Lagrange-basis data. The roughly 27× wasm-to-native gap in verification has not
been profiled yet.
