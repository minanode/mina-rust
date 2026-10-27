# ledger-tool

Writes the genesis configs the node reads:

- a prebuilt config, in bin_prot: the protocol constants and the genesis,
  staking epoch and next epoch ledgers, with their inner hashes, so that a node
  or a replay loads them without hashing;
- with `--without-ledgers`, a JSON config with only what the genesis block
  commits to, for a node that syncs its ledgers from peers.

Either is what `mina node --genesis-config` takes, and the `--genesis` of
`mina-block-replay` takes the prebuilt one.

## From a network's RocksDB ledger tarballs

OCaml publishes the ledgers of a hard fork's genesis as tarballs of RocksDB
databases, and the network's config names them by hash, with each tarball's
SHA3-256 (`s3_data_hash`). `genesis-from-tarballs.sh` builds both configs from
them without an OCaml daemon:

```sh
cargo build --release -p ledger-tool
tools/ledger-tool/genesis-from-tarballs.sh <OCaml config> <out dir> <name>
```

1. `ledger-tool --input <config> --tarball-urls` prints where each ledger's
   tarball is: OCaml names it after the ledger hash and the bin_prot empty
   account.
2. The script downloads each tarball, checks its SHA3-256 against the config,
   and dumps its database with OCaml's `mina-rocksdb-scanner`, through
   `scripts/ocaml-mina`.
3. `ledger-tool --input <config> --genesis-dump … --staking-dump … --next-dump … --output <name>.bin`
   reads the accounts from the dumps in ledger index order, checks the root each
   database stores against the config, rebuilds each ledger, checks its root
   again, and writes the prebuilt config. The protocol constants and seeds come
   from the config.
4. `ledger-tool --prebuilt <name>.bin --without-ledgers --output <name>-without-ledgers.json`
   writes the JSON config.

For Mesa devnet, with the config of the OCaml 4.0.2 devnet deb, this takes about
4 minutes and writes a 239 MB `mesa-devnet.bin`; most of it is hashing the three
ledgers of about 92,000 accounts each. The JSON config is embedded as
`genesis_ledgers/mesa-devnet-without-ledgers.json`.

## From a daemon.json with accounts

```sh
cargo run --release --bin ledger-tool -- --input daemon.json --output genesis.bin
```

The config must list every account of its three ledgers, as the Mesa mainnet
genesis on the development machine does (`mesa-mainnet-daemon.json`, exported
from an OCaml node). `--url` downloads the config instead.

## Canonical milestone artifacts

Regeneration is checked by ledger roots, not byte identity with old artifacts.
Current files preserve runtime `proof`/`daemon` metadata and a versioned binary
header. The canonical mainnet bootstrap JSON is checked into `genesis_ledgers/`.
See
[the regression root check](../regression/README.md#regenerated-genesis-versus-canonical-bytes).

To recompute roots from the accounts in a prebuilt binary without trusting its
stored roots or cached hashes:

```sh
ledger-tool --prebuilt genesis.bin --recompute-ledger-roots --output roots.json
```

This writes only the three computed roots. Compare them to independently pinned
expected roots, as `inputs.py check --genesis-roots` does for mainnet. Normal
`--without-ledgers` conversion uses the caches for startup speed and is not an
account-integrity check.
