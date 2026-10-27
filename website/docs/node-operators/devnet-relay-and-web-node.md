---
sidebar_position: 3
title: Devnet node, relay and web node
description:
  Run a Rust node on Mesa devnet, make it a relay for browsers, and bootstrap a
  web node on devnet through it
slug: /node-operators/devnet-relay-and-web-node
---

# Devnet node, relay and web node

This recipe runs three things on one machine:

1. a native Rust node that follows Mesa devnet, the network the OCaml node 4.0.2
   runs (chain ID
   `ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8`);
2. the same node as a **relay** for browsers: it serves WebRTC signaling on its
   HTTP port, so web nodes can join devnet through it;
3. a **web node** built for devnet, which bootstraps through that relay in a
   browser.

It also keeps the node running and records its health. The scripts are in
[`tools/regression/devnet/`](https://github.com/o1-labs/mina-rust/tree/develop/tools/regression/devnet).
The numbers below are from the development machine on 2026-10-21 (Linux x86_64,
62 GB of memory).

## What you need

- **The node,** built from source: `cargo build --release --bin mina` gives
  `target/release/mina` (see [Building from source](building-from-source)).
  `mina build-info` says which commit it is.
- **The devnet genesis,** which needs nothing extra:
  `mina node --network devnet` embeds the Mesa devnet genesis config without
  ledgers, `genesis_ledgers/mesa-devnet-without-ledgers.json` (6,230 bytes,
  sha256 `5bfca7c1…`). Devnet is past its genesis, so the node syncs its ledgers
  from peers. The prebuilt config with the three ledgers (`mesa-devnet.bin`, 239
  MB), written by `tools/ledger-tool/genesis-from-tarballs.sh` from OCaml's
  ledger tarballs, is only for replays and tests; `genesis_ledgers/README.md`
  says how to make it.
- **For the web node:** Node.js and the frontend's dependencies (`npm ci` in
  `frontend/`), the web node's WASM (below), and a static server that sends the
  cross-origin isolation headers the WASM threads need (`npx serve@14` with
  `tools/regression/webnode/serve.json`).

## Ports

| What                                 | Default | On the development machine |
| ------------------------------------ | ------- | -------------------------- |
| Node HTTP: RPC and WebRTC signaling  | 3000    | 3540                       |
| Node libp2p, to and from OCaml peers | 8302    | 8740                       |
| Static server for the web node build | -       | 4224                       |

Browsers reach the relay on its HTTP port. OCaml peers reach the node on its
libp2p port if it is open; the node also connects out to devnet's seeds
(`seed-1` to `seed-3.devnet.gcp.o1test.net`, the Mesa devnet config's peer list)
and the peers they give it.

## 1. Run a devnet node

```bash
mina node --network devnet -d ~/.mina-devnet -p 3000 --libp2p-port 8302
```

or with the script, which keeps the node's files in one directory:

```bash
MINA=target/release/mina tools/regression/devnet/node.sh ~/devnet-node 3000 8302 \
    > devnet-node.log 2>&1 &
```

It syncs in about a minute: the relay on the development machine logged `Synced`
54 s after it started, and P1c's devnet run took 81 s. Check it:

```bash
curl -s localhost:3000/status | jq '{chain_id, sync: .transition_frontier.sync.status,
  tip: .transition_frontier.best_tip.height,
  peers: [.peers[] | select(.connection_status == "Connected")] | length}'
```

`chain_id` must be `ebfce0d5…` and `sync` `Synced`; the tip should match
devnet's (the connected peers report theirs in `.peers[].best_tip_height`).

## 2. Make it a relay

A relay is the same node with a fixed peer ID, so that the address compiled into
a web build stays valid across restarts. Give `node.sh` a key file as its fourth
argument; it makes one (mode 600) if the file is missing, passes it to the node
in `MINA_P2P_SEC_KEY` and writes the peer ID to `RUN_DIR/peer-id`.

Stop step 1's node first (Ctrl-C, or `kill` its PID): the relay takes the same
ports, and a browser would otherwise reach the old node under the relay's peer
ID. Then start the relay and write its PID where the watchdog (step 5) looks for
it:

```bash
mkdir -p ~/devnet
MINA=target/release/mina nohup tools/regression/devnet/node.sh ~/devnet-relay \
    3000 8302 ~/devnet-relay.key >> ~/devnet/native-devnet.log 2>&1 &
echo $! > ~/devnet/native-devnet.pid
cat ~/devnet-relay/peer-id
```

`node.sh` becomes the node, so `$!` is the node's PID. You can also skip this
command and let the watchdog start the relay: step 5 with the same settings
starts it when nothing holds the ports.

The relay's WebRTC address is
`/ip4/<its IP>/tcp/<HTTP port>/webrtc/http/p2p/<peer ID>`, for example
`/ip4/127.0.0.1/tcp/3000/webrtc/http/p2p/12D3KooW…` for a browser on the same
machine. Keep the key file private and keep it: a new key means a new peer ID
and a new web build.

## 3. Build the devnet web node

The **WASM** is the same for mainnet and devnet (it picks the network at run
time). Build it as in the
[regression harness README](https://github.com/o1-labs/mina-rust/tree/develop/tools/regression#building-and-serving-a-web-node),
step 1, which gives a `pkg` directory. Then build the **frontend** for devnet
with the relay as its bootnode, and serve it:

```bash
RELAY=/ip4/127.0.0.1/tcp/3000/webrtc/http/p2p/$(cat ~/devnet-relay/peer-id)
CIRCUIT_CACHE=~/devnet-relay/home-env/.mina/circuit-blobs/berkeley-devnet \
    tools/regression/devnet/webnode-build.sh ~/devnet-web "$RELAY" 4224 pkg
cd ~/devnet-web/browser && npx -y serve@14 -s . -l 4224 --no-port-switching -c serve.json
```

`webnode-build.sh` compiles in the network (`devnet`), the genesis URL
(`/webnode-devnet/genesis.json`, the config without ledgers), the relay as the
bootnode and `http://localhost:4224/webnode-devnet/peers.txt` as the seed list,
and prints what it compiled in and the hashes of the WASM, the genesis config
and the verifier indexes. `CIRCUIT_CACHE` copies the verifier indexes the native
node made, which saves the browser from making its own.

## 4. Open the web node

Open `http://localhost:4224/loading-web-node` in Chrome and choose **Start the
web node** (**Start Non BP**, a node that doesn't produce blocks, in a build
that shows block production controls). It connects to the relay over WebRTC and
syncs devnet's ledgers and best chain from it; the relay follows devnet's OCaml
peers. In the browser's console, `await webnode.status()` shows its sync status,
best tip and peers.

On the development machine, in headless Chrome, a fresh web node reported
`Synced` 62 s after the page opened, at the relay's tip, using about 1.7 GB of
memory (1.0 GB of it WASM). In another run it took 177 s: its first ledger query
on each of three connections to the relay went unanswered and timed out, and it
reconnected each time. The P1f note has both runs. While a web node is
connected, the relay's `/status` lists it as a peer with `"is_libp2p": false`.

## 5. Keep it running

`tools/regression/devnet/watch.sh` is a watchdog for the node: it adopts the one
in its PID file, or the one listening on the node's ports, restarts it only when
its process has gone (at most `MAX_RESTARTS`, default 5, times an hour), moves
the old log aside on a restart, and every minute appends a JSON line from
`/status` to a health log. It never starts a node while something answers on the
HTTP port or listens on the libp2p port, and only one watchdog runs for a PID
file. With `SERVE_DIR` and `SERVE_PORT` it keeps the static server up too,
starting one only when nothing holds the port. Its settings are environment
variables (`watch.sh --help`):

```bash
export MINA=$HOME/devnet/bin/mina          # a pinned copy, not target/release
export GENESIS=$PWD/genesis_ledgers/mesa-devnet-without-ledgers.json
export RUN_DIR=$HOME/devnet-relay HTTP=3000 P2P=8302 KEY=$HOME/devnet-relay.key
export LOG=$HOME/devnet/native-devnet.log PIDFILE=$HOME/devnet/native-devnet.pid
export HEALTH=$HOME/devnet/native-devnet.health
export SERVE_DIR=$HOME/devnet-web/browser SERVE_PORT=4224
export SERVE_PIDFILE=$HOME/devnet/static-4224.pid
setsid nohup tools/regression/devnet/watch.sh >> $HOME/devnet/watch.out 2>&1 &
```

Copy the binary to a fixed path first: the watchdog restarts the node from
`MINA`, and a rebuild in a checkout would otherwise change what it runs. Stop
the watchdog by its PID; that leaves the node and the static server running. Its
first lines in `~/devnet/native-devnet.events` say which node it adopted;
`tools/regression/devnet/test-watch.sh` tests it with stand-in processes.

To see how the node has been:

```bash
python3 tools/regression/devnet/summary.py $HOME/devnet/native-devnet.health
python3 tools/regression/soak_health.py $HOME/devnet/native-devnet.log
```

`summary.py` prints one row per hour: samples, how many were `Synced`, the best
tip heights, how many blocks the node was behind its highest peer at most,
connected peers, memory, and unreadable samples, then the watchdog's events.
`soak_health.py` counts the log's warnings and errors by kind; it needs the
kinds logged at `debug`, so run the node with `--verbosity debug` (about 6 MB an
hour on devnet) if you use it. At the default level a synced node logs about 20
to 40 KB an hour, under 1 MB a day; see
[Logs and log levels](node-management#logs-and-log-levels).

## Troubleshooting

- **The web node finds no peers:** check that the relay runs and that the peer
  ID in the build (`webnode-devnet/peers.txt` in the build) is the relay's
  (`RUN_DIR/peer-id`); and that the page is served with
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp` (`curl -sI` the page).
- **The node stays at `Bootstrap`:** look in its log for
  `TransitionFrontierSyncLedger` progress and for peers; devnet has few OCaml
  peers, so a node that cannot reach the seeds cannot sync.
- **Several web nodes:** web nodes bootstrapping through one relay at the same
  time connect to each other through it, which slows each.
- **A public relay:** the build's seed URL is `http://localhost:<port>/…` and
  the relay address is whatever you give `webnode-build.sh`, so serving the web
  node to other machines needs a public relay address (`/ip4/<public IP>/…` or
  `/dns4/<name>/…`) and a seed URL on the public origin; this recipe has only
  been run with browser, relay and server on one machine.
