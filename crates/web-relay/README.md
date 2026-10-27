# mina-web-relay

Forward Mina blocks from libp2p gossip to browsers over WebSocket.

Browsers cannot join Mina's libp2p network directly. The relay joins it for
them: it connects to a network's seeds, discovers further peers with Kademlia on
`/coda/kad/1.0.0`, subscribes to the consensus gossip topic, and forwards every
block message unchanged. It verifies nothing. The browser
([mina-web-verifier](../web-verifier)) checks each block's proof and runs fork
choice itself, so a relay cannot make a client accept an invalid block; it can
only withhold or delay blocks, which the client detects as a stale tip. Running
several independent relays guards against that.

The gossip loop is adapted from
[o1-labs/mina-light-node](https://github.com/o1-labs/mina-light-node)
(`crates/mina-relay`, Apache-2.0), on the workspace's `mina-transport`.

## Usage

```sh
cargo run --release -p mina-web-relay -- --network mainnet --listen 127.0.0.1:8741
```

| Endpoint      | What it serves                                                                                            |
| ------------- | --------------------------------------------------------------------------------------------------------- |
| `GET /blocks` | WebSocket. Sends the cached recent blocks, then each new one, as binary messages with the gossip payload. |
| `GET /status` | JSON: network, chain id, connected peers, clients, cached and relayed block counts, last block time.      |

Networks are the Mesa mainnet and devnet, with chain ids from
`mina internal chain-id` (4.0.2) and seeds from the published seed lists. Add
peers with `--peer <multiaddr>`.

A payload is `[8-byte little-endian length][GossipNetMessageV2]`, and a block is
the `NewState` variant (tag 0), as `Verifier::verify_gossip` expects.
