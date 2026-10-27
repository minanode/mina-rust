# Genesis configs

## `mesa-devnet-without-ledgers.json`

The Mesa devnet genesis, without its ledgers: the protocol constants, the
genesis, staking epoch and next epoch ledger hashes, their total currencies, the
epoch seeds, and the genesis block producer's stake proof. It is what the
genesis block commits to, and `DEVNET_CONFIG` (`crates/node/src/config.rs`)
embeds it: it is the genesis of `mina node --network devnet` without
`--genesis-config` or `--config`, and of a devnet web node without a genesis
URL. Devnet has moved past its genesis, so a node starting from it syncs its
ledgers from peers.

It replaces the Berkeley devnet `devnet.bin`, a prebuilt config with ledgers. A
Mesa prebuilt config is 239 MB, too big to embed; the command below writes one
into its output directory.

It is written by
[`tools/ledger-tool/genesis-from-tarballs.sh`](../tools/ledger-tool/genesis-from-tarballs.sh)
from the RocksDB ledger tarballs of the Mesa devnet config of OCaml 4.0.2
(`genesis_ledgers/devnet.json` in the OCaml repository at `84b8d969`, and
`config_84b8d969.json` in the mina-devnet-config deb), with OCaml's
`mina-rocksdb-scanner` and `ledger-tool`, no OCaml daemon:

```sh
cargo build --release -p ledger-tool
tools/ledger-tool/genesis-from-tarballs.sh <devnet.json> <out dir> mesa-devnet
cp <out dir>/mesa-devnet-without-ledgers.json genesis_ledgers/
```

The prebuilt config with the ledgers, `<out dir>/mesa-devnet.bin`, is for
replays and tests. See [`tools/ledger-tool`](../tools/ledger-tool/README.md).

## `mesa-mainnet-without-ledgers.json`

The Mesa mainnet genesis, without its ledgers, in the same form, plus the daemon
JSON's `proof` (the fork point) and `daemon` sections. A node started with
`--genesis-config genesis_ledgers/mesa-mainnet-without-ledgers.json` has the
genesis, the chain id, the constants and the daemon settings of one started with
OCaml's full mainnet daemon JSON (`--config`, 736 MB), without parsing it or
loading its three genesis ledgers (20 to 26 s): mainnet has moved past its
genesis, so a node syncs its ledgers from peers, or restores them from its
frontier store. `crates/node/tests/compact_mainnet_genesis.rs` checks it against
OCaml's packaged 4.0.2 mainnet config and the genesis block a 4.0.2 daemon
serves (I5d2, private implementation notes).

It was written by `ledger-tool` from the daemon JSON with the three ledgers
(OCaml's `config_84b8d969.json` with the accounts of its ledger tarballs), each
ledger's root checked against the config's hash:

```sh
cargo build --release -p ledger-tool
target/release/ledger-tool --input mesa-mainnet-daemon.json --without-ledgers \
  --output genesis_ledgers/mesa-mainnet-without-ledgers.json
```
