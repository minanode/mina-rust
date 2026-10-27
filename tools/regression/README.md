# Regression harness

Tools to measure a change against recorded baselines and reproduce the checks:

- [`baseline.py`](baseline.py) prints the baseline table from fresh
  measurements: full replay, staged ledger rebuild, and, from logs it is given,
  browser and native bootstraps, and the soak node's state.
- The fast replay subset,
  [`tools/block-replay/tests/replay_subset.rs`](../block-replay/tests/replay_subset.rs),
  replays Mesa mainnet to just inside its first zkApp-heavy stretch in under 3
  minutes, every staged ledger hash checked.
  [`block_density.py`](block_density.py) counts what each block carries, and
  [`zkapp_shapes.py`](zkapp_shapes.py) which kinds of zkApp command a chain
  holds, to choose such subsets.
- [Replays from a dump](#replays-from-a-dump) start from a staged ledger dump
  instead of genesis, so they can cover any stretch of the chain:
  [`replay_from_dump.rs`](../block-replay/tests/replay_from_dump.rs) replays the
  last 1,000 blocks of the replay chain.
- [`inputs.py`](inputs.py) checks the inputs against the manifests in
  [`inputs/`](inputs), their sizes and sha256 checksums, so they can be copied
  to another machine ([Checking the inputs](#checking-the-inputs)).

- [Devnet replays](#devnet-replays): the devnet replay subset,
  [`devnet_replay_subset.rs`](../block-replay/tests/devnet_replay_subset.rs),
  replays Mesa devnet's first blocks from genesis and recent ones from a node's
  root in about a minute. The bucket has lost the blocks in between.
- [Devnet: a relay and a web node](#devnet-a-relay-and-a-web-node):
  [`devnet/`](devnet) runs a native devnet node or relay under a watchdog that
  records its health, and builds a web node that bootstraps devnet through it.
- [`pc3-kill/`](pc3-kill) SIGKILLs a devnet follower at named points of a
  bootstrap, a frontier-store checkpoint or a refreeze and checks each restart
  against a reference node (`kill_loop.py`, `summarize.py`), and runs a follower
  under a 1 GB memory cap sampled by phase (`mem_run.py`) (PC3-KILL note).
- [`webnode/`](webnode) drives a web node in headless Chrome: `run.mjs` starts
  one and records its bootstrap, and `watch.mjs`, `peek.mjs`, `profile.mjs` and
  `shot.mjs` attach to a running one.
- [A native bootstrap](#a-native-bootstrap) is a command to run by hand, not a
  script: it joins mainnet, which the rules for the shared machine limit.
- [`lock.sh`](lock.sh) takes the shared machine's browser lock or one of its two
  node slots before running a command; `run.mjs` takes the browser lock itself
  ([Taking turns](#taking-turns)).
- [`verify_alert.py`](verify_alert.py) reads a running node's log from where it
  last stopped and prints an alert if a transaction proof failed verification
  (`verify_transactions OK=false`): a key or verifier regression shows up live.
  It is meant for the soak watchdog's loop (P3 note).

Everything is parameterised by flags or environment variables. Nothing here
starts a node that joins mainnet; a browser bootstrap needs a relay that has.

## Inputs that are not in the repo

| Input                     | Used by                   | Where it comes from                                                                                                                                                                                                                                            |
| ------------------------- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Replay blocks, chain file | replays                   | [`fetch_chain.py`](../block-replay/fetch_chain.py), below                                                                                                                                                                                                      |
| `mesa-mainnet.bin`        | replays from genesis      | `ledger-tool --input mesa-mainnet-daemon.json --output mesa-mainnet.bin`, from the Mesa `daemon.json` with its three ledgers exported from an OCaml 4.0.2 node (255 MB); `genesis-from-tarballs.sh` checks the same ledger roots, with a newer metadata format |
| A staged ledger dump      | rebuild, replay from dump | A node run with `MINA_DUMP_RECONSTRUCT` set writes `<data dir>/debug/failed_reconstruct_ctx.binprot` (about 135 MB); `mina-block-replay --write-dump` writes one too                                                                                           |
| A web node build          | browser bootstrap         | Built as below, or a relay's own build                                                                                                                                                                                                                         |
| A relay                   | browser bootstrap         | A native node following mainnet, with WebRTC signaling on its HTTP port                                                                                                                                                                                        |
| A native bootstrap log    | native bootstrap row      | [A native bootstrap](#a-native-bootstrap), below                                                                                                                                                                                                               |
| A running soak node       | soak row                  | A native node following mainnet; on the development machine the soak node on port 3200, run by `$WORK/soak/watch.sh`                                                                                                                                           |
| `mesa-devnet.bin`         | devnet genesis head       | [`ledger-tool/genesis-from-tarballs.sh`](../ledger-tool/README.md), from the Mesa devnet config and its RocksDB ledger tarballs (239 MB)                                                                                                                       |
| Devnet blocks, chain file | devnet replays            | `fetch_chain.py --network devnet`, [below](#devnet-replays); the first 7 Mesa blocks from R13's download                                                                                                                                                       |
| A devnet root dump        | devnet replay subset      | `devnet-root-dump.sh` (private helper, not distributed): a devnet node bootstrapped once with `MINA_DUMP_RECONSTRUCT` set (105 MB)                                                                                                                             |

On the development machine the inputs are in the main checkout's `.work/`. This
README calls the main checkout `$ROOT` and its `.work/` `$WORK`; from the main
checkout or any of its worktrees:

```bash
ROOT=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
WORK=$ROOT/.work
```

`$WORK` there holds `replay/` (`chain.txt` and `blocks/`, every block to
564044), `replay-devnet/` (devnet, below), `genesis-mainnet/mesa-mainnet.bin`,
`genesis-mainnet/mesa-mainnet-without-ledgers.json` (for a native bootstrap and
a web node build), `genesis-devnet/mesa-devnet.bin` and
`genesis-devnet/mesa-devnet-without-ledgers.json` (the same for devnet; the
latter is embedded as the default devnet genesis), `reconstruct-current.binprot`
and, for the [replay from a dump](#replays-from-a-dump),
`dumps/563044-3NLxQsoFuJC2oBWyCwRNBXqJy32Ywang7MHQ9RosrVWFHnKsFuhL.binprot`. The
test relay serves its web node build on port 4202. `$REPO` is your own checkout
or worktree; commands without it run from there.

### Fetching the replay blocks

Blocks come from the public `mina_network_block_data` bucket. `fetch_chain.py`
walks back from a tip to a starting height and downloads that chain,
zstd-compressed (it needs the `zstd` command). Mesa mainnet's genesis is 548147,
so replays start at 548148.

The subset only needs the chain to 551260: 3,113 blocks, 954 MB compressed.

```bash
python3 tools/block-replay/fetch_chain.py --from-height 548148 --tip-height 551260 \
    --tip 3NKAGGsspiboutUBQxoJ8ibVfMJszsYuWz7TWAqTeGaceohbqGcC --out replay
```

The full replay in the baseline is the chain to 564044: 15,897 blocks, 5.5 GB
compressed.

```bash
python3 tools/block-replay/fetch_chain.py --from-height 548148 --tip-height 564044 \
    --tip 3NL9ixEeoyrPa8nLyyKsRnHNuMg6yoaztdFdK5ZbRmJkPqqBE5Hd --out replay
```

Either writes `replay/chain.txt` and the blocks next to it, all in the `--out`
directory. The test and `baseline.py` also accept the blocks in a `blocks/`
subdirectory, as in `$WORK/replay`.

To replay further, run it again with a newer tip. It always lists the bucket and
rewrites `chain.txt`. It reads the first 2 KB of each candidate block for its
parent only once, keeping the parents in `<out>/parents.txt`, and skips any
block whose `.json.zst` is already in `--out`. So `--out` must be the directory
that holds the blocks: with the `blocks/` layout, pass `--out replay/blocks` and
then move `replay/blocks/chain.txt` over `replay/chain.txt`. Pointed at `replay`
instead, it downloads every block again next to `blocks/`. If the walk back from
the tip stops short of `--from-height`, it says which block's parent is missing
from the bucket; `--genesis <state hash>` checks the block the chain starts
from.

## Binaries

### Seed a worktree target and check provenance

Use `seed-target.sh` to reuse an idle donor's Cargo artifacts. The destination
must be absent, on the same filesystem, and stay idle until seeding finishes.
The helper gives mutable Cargo state private inodes, removes copied locks and
incremental sessions, and invalidates local workspace fingerprints. It prints
copy/allocation counts and reserves 55 GiB free disk. The donor must be outside
the destination checkout. Both seeding and repair privately copy multiply-linked
regular files outside the target too, including tracked and ignored generated
sources, and verify that no source links remain. The scan covers the nearest
ancestor checkout with `.git` (or the target's parent for standalone scratch
trees), excluding Git metadata, symlink traversal and separate nested checkouts.
Keep the entire destination checkout idle.

```bash
tools/regression/seed-target.sh /absolute/donor/target "$PWD/target"
build_expected_commit=$(git rev-parse HEAD)
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
    cargo build --release -p mina-cli --bin mina
tools/regression/check-build-env.sh target/release/mina "$build_expected_commit"
```

`check-build-env.sh` reads the native binary's `build-info` and SHA-256 without
starting a node. It exits 1 for a commit mismatch and 2 if identity cannot be
checked. Pin that checked hash with gate results. A matching embedded node stamp
alone does not attest every linked crate or an unstamped test/helper executable.

For an existing target, use `seed-target.sh --repair /absolute/worktree/target`.
It acquires all three regression build slots without waiting, checks Cargo locks
and `/proc`, and exits 75 if busy. It changes retained files' inodes while
preserving their bytes, modes and mtimes; it discards only regenerable workspace
fingerprints, incremental sessions and locks. It does not correct a previously
contaminated binary. Run the next build and provenance check before using it.
`--dry-run` inspects either operation. See the T1 report for the measured
rewrite rules, repair log and historical gate audit.

### Replay tools

Build them in your checkout, or use the ones already built from `main` (on the
development machine, in `$ROOT/target/release`).

```bash
cargo build --release -p mina-block-replay --features profile \
    --bin mina-block-replay --example profile_reconstruct --example check_chain
```

`check_chain` reads every block of a chain without applying any: each must parse
with every zkApp account update digest matching OCaml's, hash to the state hash
in its name, and have the line before it as its parent. It reads the 15,897
mainnet blocks to 564044 in 46 s with 8 threads:

```bash
RAYON_NUM_THREADS=8 target/release/examples/check_chain --network mainnet \
    --chain $WORK/replay/chain.txt --blocks $WORK/replay/blocks
```

## The fast replay subset

It replays from genesis to 551260, 3,113 blocks; a
[replay from a dump](#replays-from-a-dump) can start later. The first 3,095
blocks are mostly payments, with 21 zkApp commands and two stretches of over
5,000 signed commands per 250 blocks. The last 18 are the start of the chain's
first zkApp-heavy stretch, 551243 to 551296: 213 zkApp commands with 6,603
account updates. It checks every block's staged ledger hash and the state hash
it ends at.

```bash
MINA_REPLAY_DIR=$WORK/replay \
MINA_GENESIS_BIN=$WORK/genesis-mainnet/mesa-mainnet.bin \
cargo test --release -p mina-block-replay --test replay_subset -- --ignored --nocapture
```

It is `#[ignore]`d because it needs these inputs, and it refuses to run without
`--release`. It prints the replay's summary and how long it took, and a note if
that was over 3 minutes.

Replays from genesis on the development machine (32 cores), with `main`'s
binary; "replay" is the time the binary reports, "wall" includes loading the
genesis ledger:

| To     | Blocks | zkApp commands | Replay | Wall  | Load average |
| ------ | ------ | -------------- | ------ | ----- | ------------ |
| 551260 | 3,113  | 234            | 122 s  | 126 s | about 1      |
| 551260 | 3,113  | 234            | 123 s  | 127 s | 2 to 3.5     |
| 551260 | 3,113  | 234            | 141 s  | 145 s | 13 to 17     |
| 551300 | 3,153  | 609            | 158 s  | 162 s | 0.7 to 3.5   |
| 551300 | 3,153  | 609            | 181 s  | 185 s | about 10     |
| 551270 | 3,123  | 323            | 173 s  | 177 s | about 34     |

Replaying the whole first stretch fits in 3 minutes on a quiet machine but not
once the load reaches about 10, which is common on the shared machine; the
subset ends at 551260 to keep that margin. Little is lost: the stretch repeats
one kind of zkApp command. [`zkapp_shapes.py`](zkapp_shapes.py) compares each
command's call tree, with every account update's authorization kind, token,
balance sign, updated fields, account preconditions and event and action counts.
587 of the stretch's 588 commands have the same shape, and all one fee payer. In
the chain to 564044 there are 19 shapes among 3,835 zkApp commands; the subset
has 8 of them, which 3,820 of the commands share, and the other 11 appear later,
one to three commands each.

The replay of every block, the full gate in the historical baseline, runs the
same binary without `--to`; `baseline.py` runs and times it.

To choose another subset, for example once the chain has grown,
[`block_density.py`](block_density.py) counts the zkApp commands, account
updates and signed commands in every block of a chain. In the blocks to 564044,
365 of 15,897 blocks carry zkApp commands. Nearly all of them come in six
stretches of about 50 blocks with about 590 zkApp commands each, starting at
551243, 553905, 556641, 559004, 561569 and 563859. On a quiet machine a block of
such a stretch takes about 1 s to replay: the 40 blocks from 551261 to 551300,
34 of them carrying 375 zkApp commands between them, added 35 s. Other blocks
take 0.01 to 0.14 s, by their signed commands: in the full replay's log, 250
blocks with 278 signed commands took 3 s, and 250 with 7,486 took 35 s.

## Replays from a dump

`mina-block-replay --from-dump DUMP` starts from a staged ledger dump instead of
genesis: the format a node writes with `MINA_DUMP_RECONSTRUCT`, which holds a
block's snarked ledger, scan state, pending coinbase and the protocol states the
scan state needs. The replay builds the snarked ledger, rebuilds the staged
ledger from it as a node does after syncing its root, and finds the block it
belongs to in the chain file: the first, from the height of the newest protocol
state the dump holds, with the rebuilt staged ledger's hash. That block has to
be in the blocks directory too, for its protocol state. It then applies the
blocks after it as a replay from genesis does, checking every staged ledger
hash.

`--write-dump FILE` writes a dump of the last block a replay applied, in the
same format. The replay then keeps the snarked ledger as well, as a node does:
when a block emits a ledger proof, it applies the proof's transactions to the
snarked ledger and checks the result against the block's snarked ledger hash. So
a replay from genesis can make a dump at any height, and a replay from a dump
can make a later one:

```bash
mina-block-replay --genesis $WORK/genesis-mainnet/mesa-mainnet.bin \
    --chain $WORK/replay/chain.txt --blocks $WORK/replay/blocks \
    --to 563044 --write-dump $WORK/dumps/563044-3NLxQsoFuJC2oBWyCwRNBXqJy32Ywang7MHQ9RosrVWFHnKsFuhL.binprot
```

The test, [`replay_from_dump.rs`](../block-replay/tests/replay_from_dump.rs),
replays the last 1,000 blocks of the replay chain, from that dump of 563044 to
564044, every staged ledger hash checked. A second test there,
`dump_round_trip`, writes dumps from short replays near genesis and checks that
a replay started from one ends in the same dump, byte for byte, as a replay from
genesis.

```bash
MINA_REPLAY_DIR=$WORK/replay \
MINA_REPLAY_DUMP=$WORK/dumps/563044-3NLxQsoFuJC2oBWyCwRNBXqJy32Ywang7MHQ9RosrVWFHnKsFuhL.binprot \
MINA_GENESIS_BIN=$WORK/genesis-mainnet/mesa-mainnet.bin \
cargo test --release -p mina-block-replay --test replay_from_dump -- --ignored --nocapture
```

On the development machine (32 cores), with the branch's binary on 2026-10-20.
"Start" is loading genesis, or reading the dump and rebuilding its staged
ledger; "replay" is the time the binary reports for applying the blocks; "wall"
includes both. The first two rows ran at the same time:

| From                         | To     | Blocks | Start | Replay        | Wall           | Load average |
| ---------------------------- | ------ | ------ | ----- | ------------- | -------------- | ------------ |
| genesis                      | 564044 | 15,897 | 4 s   | 1,150 s       | 1,154 s        | 11 to 14     |
| genesis, with `--write-dump` | 563044 | 14,897 | 4 s   | 1,309 s       | 1,315 s        | 7 to 13      |
| the dump of 563044 (test)    | 564044 | 1,000  | 17 s  | 91 s to 111 s | 107 s to 129 s | 2 to 15      |
| the relay's dump of 563941   | 564044 | 103    | 29 s  | 20 s          | 49 s           | 5 to 6       |

Starting from a dump takes about 1 s to read it, 10 s to build the snarked
ledger, whose 290,000 accounts are hashed in parallel, and 5 s to rebuild the
staged ledger of 563044, or 19 to 25 s for the relay's dump of 563941, whose
scan state holds the zkApp-heavy stretch from 563859. Keeping the snarked ledger
for `--write-dump` makes a replay about a quarter slower (1,304 s against 1,039
s for the first 14,750 blocks, side by side) and doubles its peak memory, from
1.0 GB to 1.9 GB; writing the dump takes 1 to 2 s. The dump of 563044 is 95 MB,
the relay's of 563941 137 MB.

A replay's dump holds the same staged ledger as a node's dump of the same block:
the replay's dump of 563941, from the dump of 563044, has the same accounts,
scan state, pending coinbase and protocol states as the test relay's, the last
two in another order.
[`compare_dumps`](../block-replay/examples/compare_dumps.rs) compares two dumps
part by part:

```bash
cargo run --release -p mina-block-replay --example compare_dumps -- A.binprot B.binprot
```

To move the dump forward when the chain grows, replay from it with
`--write-dump`, about 2 minutes per 1,000 blocks, then write a new manifest
([below](#checking-the-inputs)) and update `FROM` and `TO` in the test.

## Checking the inputs

The manifests in [`inputs/`](inputs) list the inputs above that are files, with
their sizes and sha256 checksums, one set per manifest, paths relative to the
inputs directory (`$WORK` here):

| Manifest                                              | Files                                                                                     | Size   |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------- | ------ |
| [`genesis.txt`](inputs/genesis.txt)                   | `mesa-mainnet.bin`, and `mesa-mainnet-without-ledgers.json` for bootstraps and web builds | 256 MB |
| [`replay-subset.txt`](inputs/replay-subset.txt)       | the fast replay subset's 3,113 blocks                                                     | 954 MB |
| [`replay-from-dump.txt`](inputs/replay-from-dump.txt) | the dump of 563044 and the 1,001 blocks from 563044 to 564044                             | 468 MB |
| [`rebuild.txt`](inputs/rebuild.txt)                   | `reconstruct-current.binprot`, the rebuild row's dump                                     | 137 MB |

[`inputs.py`](inputs.py) checks files against them, and only needs Python:

```bash
tools/regression/inputs.py check tools/regression/inputs/*.txt
tools/regression/inputs.py check --dir DIR --chain DIR/replay/chain.txt tools/regression/inputs/replay-from-dump.txt
tools/regression/inputs.py check --sizes-only tools/regression/inputs/*.txt
```

`--dir` defaults to the main checkout's `.work/`. `--chain` also checks that the
chain file lists every block in the manifests at its height. It prints a line
per manifest, and each file that is missing or differs, and exits with status 1
if any does. Checking all four, 4,118 files and 1.8 GB, takes 0.8 s here with
the files in the page cache, and `--sizes-only` 0.1 s.

The small mainnet JSON is checked into `genesis_ledgers/`. The binary and dumps
await publication. Public blocks can be downloaded as above. To use supplied
fixtures, place the files with the same layout under one directory, pass it as
`--dir`, and write the chain file from the manifests, since
`$WORK/replay/chain.txt` grows when the chain is fetched further. Both tests can
share it: the subset stops at 551260, and the replay from the dump skips the
lines up to 563044.

```bash
tools/regression/inputs.py chain tools/regression/inputs/replay-subset.txt \
    tools/regression/inputs/replay-from-dump.txt > DIR/replay/chain.txt
```

### Regenerated genesis versus canonical bytes

`genesis.txt` pins the historical binary and JSON used for the milestone
receipts. The checked-in 6,222-byte JSON is the canonical bootstrap artifact. Do
not replace those hashes with the output of a new generator.

Current `ledger-tool` preserves `proof` and `daemon` runtime metadata (P11). The
JSON gains 513 bytes; the binary gains a 526-byte `MINAGEN1` header (8-byte
magic, 4-byte length and 514 bytes of metadata). This is a format-version
change, not whitespace or field order. The three ledger roots remain the same.
The legacy payload remains readable using the selected network's compiled
defaults.

After `genesis-from-tarballs.sh` has checked tarball digests, database roots and
rebuilt account roots, check the regenerated artifacts by roots explicitly:

```bash
tools/regression/inputs.py check --dir "$WORK" --genesis-roots \
  --ledger-tool "$PWD/target/release/ledger-tool" tools/regression/inputs/genesis.txt
```

This rebuilds each binary ledger from its accounts using
`ledger-tool --recompute-ledger-roots`, ignoring every stored root and cached
inner hash. All three computed roots must match the pins in
[`genesis_ledgers/mesa-mainnet-without-ledgers.json`](../../genesis_ledgers/mesa-mainnet-without-ledgers.json),
the bootstrap artifact used by our nodes. The same roots are independently
recorded from OCaml 4.0.2's packaged mainnet config in
[`tests/files/ocaml/mainnet-4.0.2-constants.json`](../../tests/files/ocaml/mainnet-4.0.2-constants.json),
under `constants.{genesis,staking_epoch,next_epoch}_ledger_hash`, with
provenance. The companion JSON has no ledger accounts: only its declared roots
are compared to those pins. The output distinguishes these checks. It
deliberately makes no byte-identity claim. The default `check` command still
checks canonical sizes and SHA256s; use it for downloaded pinned fixtures. Root
equality does not claim historical SNARK verification or equality of every
runtime option.

The offline regression checks an untouched binary, then adds one nanomina to one
account in each ledger separately, preserving stored roots and caches:

```bash
CARGO_BUILD_JOBS=8 MINA_MAINNET_GENESIS="$WORK/genesis-mainnet/mesa-mainnet.bin" \
  tools/regression/lock.sh build -- cargo test --locked --release -p ledger-tool \
  --test genesis_roots -- --ignored --nocapture
python3 -I tools/regression/milestone/test_aux_tests.py
```

When an input changes, for example a dump at a later height, `inputs.py write`
prints a new manifest; `--blocks CHAIN FROM TO` adds the blocks from `FROM` to
`TO`:

```bash
tools/regression/inputs.py write --dir $WORK --title "..." \
    --blocks replay/chain.txt 563044 564044 dumps/563044-<state hash>.binprot
```

## Devnet replays

Mesa devnet's genesis is 545434 (`3NLT7n4L…`), so a devnet replay from genesis
starts at 545435. It can't get far for now. On 2026-10-20, from 15:58 to 22:49
UTC, every devnet object in the bucket created before 2026-09-01 was deleted, in
name order, Berkeley and Mesa alike. Its devnet blocks now start at 552017.

- The 6,582 heights from 545435 to 552016 (9,474 objects, 2.2 GB) are
  soft-deleted. A listing with `softDeleted=true` shows them, but their data
  can't be read.
- They are gone for good on 2026-10-27 at 22:48 UTC, 7 days after, unless the
  bucket's owner restores them first (P1e note).

Until then, devnet has two replays, both in the devnet replay subset:

- **The genesis head:** the first 7 Mesa blocks, 545435 to 545441, which R13
  downloaded before the deletion, replayed from the P1c genesis
  (`mesa-devnet.bin`).
- **From a root:** a devnet node's staged ledger at its root, dumped when it
  bootstrapped, then the bucket's blocks after it (`mina-block-replay --dump`).

`$WORK/replay-devnet` holds the inputs:

| File                                    | What                                                                                    |
| --------------------------------------- | --------------------------------------------------------------------------------------- |
| `chain.txt`, `<height>-<hash>.json.zst` | The chain from 552017 to 577643, 25,627 blocks, 2.9 GB, walked back from 577643         |
| `parents.txt`                           | `fetch_chain.py`'s parents of the 35,461 candidate blocks                               |
| `root-577343-3NK5JHqJ….binprot`         | The staged ledger of a node's root at 577343 (105 MB, 92,542 accounts), from 2026-10-21 |
| `genesis-head/`                         | `chain.txt` and the 7 blocks from 545435 to 545441                                      |
| `soft-deleted-545433-552016.txt`        | The deleted objects: height, state hash, generation, size, deletion times               |

### Fetching the devnet blocks

Walking back by parent hash keeps out the Berkeley blocks the bucket held at the
first Mesa heights. From the oldest block the bucket still has:

```bash
python3 tools/block-replay/fetch_chain.py --network devnet --from-height 552017 \
    --tip-height 577643 --tip 3NL1qva5UXpZB57Xz4J4PnZLu2obeUE5dkG7XLn9EaWuCRRGNM9V \
    --out $WORK/replay-devnet
```

From scratch it took 8.5 minutes: 3.7 to list the bucket and read the parents of
35,446 candidates, 4.9 to download the 25,614 blocks to 577630. Run again with a
newer tip, it took seconds. If the deleted blocks come back, start from genesis
instead:
`--from-height 545435 --genesis 3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ`.

The bucket's devnet blocks are quiet next to mainnet's. Between 552017 and
563000 there is about one zkApp command a block; after that, 1 to 439 per 1,000
blocks. In all, the chain to 577630 has 11,775 zkApp commands with 13,241
account updates, and 14,830 signed commands. After the root at 577343 there are
1 zkApp command and 186 signed commands, so for now the zkApp-heavy part is only
read (`check_chain`), not replayed:

```bash
RAYON_NUM_THREADS=8 target/release/examples/check_chain --network devnet \
    --chain $WORK/replay-devnet/chain.txt --blocks $WORK/replay-devnet
```

It reads the 25,614 blocks to 577630 in 15 s, every account update digest
matching.

### A devnet root dump

The historical measurement used a private `devnet-root-dump.sh` helper with
`MINA_DUMP_RECONSTRUCT` set. Once the node was `Synced` (108 s here), the helper
saved `/stats/sync`, whose last block is the root, and stopped the node by its
PID. That capture helper is not distributed, so this section does not provide a
public capture command. Given an existing capture, set `CAPTURE_DIR` to its run
directory and set `HEIGHT` and `STATE_HASH` from the saved root block. Copy its
dump before running the replay below:

```bash
cp "$CAPTURE_DIR/home/debug/failed_reconstruct_ctx.binprot" \
    "$WORK/replay-devnet/root-${HEIGHT}-${STATE_HASH}.binprot"
```

Then the replay starts at the root, which has to be in the chain file. It
rebuilds the root's staged ledger and checks its hash against the root block's.

```bash
target/release/mina-block-replay --network devnet \
    --dump $WORK/replay-devnet/root-577343-3NK5JHqJydM4Js9mzk6euWvsUHcgMuttG4KL6xtiXsf2jmnwwpyG.binprot \
    --root 577343 --chain $WORK/replay-devnet/chain.txt --blocks $WORK/replay-devnet
```

A wrong `--root` stops it before any block: "the dump is not the staged ledger
of …".

### The devnet replay subset

```bash
MINA_DEVNET_REPLAY_DIR=$WORK/replay-devnet \
MINA_DEVNET_GENESIS_BIN=$WORK/genesis-devnet/mesa-devnet.bin \
cargo test --release -p mina-block-replay --test devnet_replay_subset -- --ignored --nocapture
```

Its two tests, on the development machine, with load averages of 11 to 21:

| Test                   | From                   | To     | Blocks | Replay | Wall |
| ---------------------- | ---------------------- | ------ | ------ | ------ | ---- |
| `devnet_genesis_head`  | genesis 545434         | 545441 | 7      | 44 ms  | 5 s  |
| `devnet_replay_subset` | root 577343 (the dump) | 577630 | 287    | 3 s    | 53 s |

Most of the second is the rebuild: 35 s to build and hash the root's snarked
ledger and 5 s to rebuild its staged ledger (`profile_reconstruct` with
`MINA_NETWORK=devnet`).

### Verifying every devnet block

`verify_every_block` checks every proof the blocks carry, which the replay
skips: block proofs, transaction SNARKs, zkApp proofs, and the delta block chain
proof and body reference of each header (P14 note). Most of devnet's zkApp
proofs name keys set in the blocks the bucket lost, so the run takes the keys
the root dump's accounts hold, and the genesis ledger's (P14d note):

```bash
target/release/examples/verify_every_block --network devnet \
    --blocks $WORK/replay-devnet --chain $WORK/replay-devnet/chain.txt \
    --zkapp-keys-genesis $WORK/genesis-devnet/mesa-devnet.bin \
    --zkapp-keys-dump $WORK/replay-devnet/root-577343-3NK5JHqJydM4Js9mzk6euWvsUHcgMuttG4KL6xtiXsf2jmnwwpyG.binprot \
    --threads 16 --tsv results.tsv
```

`--checks block,transactions,header` and `--checks zkapps` split it in two. On
16 threads, with load averages of 19 to 82, the first took 44 minutes for the
25,627 blocks to 577643, and the second 3 hours 26 minutes for 11,519 zkApp
proofs: each costs about 18 s of CPU, as `verify_zkapp` builds an SRS and its
Lagrange basis for every proof. The genesis head is a second run, over
`$WORK/replay-devnet/genesis-head` with its `chain.txt`.

## Taking turns

The development machine is shared. Its rules (the historical baseline) allow one
browser bootstrap at a time, since each takes about 3 GB and runs through the
one test relay, and at most two extra native nodes that join mainnet, since they
all share one IP. Parallel agents also compile at most three at once, so that
memory stays clear of the live nodes. Locks keep runs within those limits:

| Lock                   | For                                                                 | Taken by                                        |
| ---------------------- | ------------------------------------------------------------------- | ----------------------------------------------- |
| `browser`              | a browser bootstrap                                                 | `webnode/run.mjs`; `lock.sh browser` for others |
| `node-1`, `node-2`     | the two node slots: one native node that joins mainnet each         | `lock.sh node`, which takes either              |
| `build-1` to `build-3` | three build slots, so parallel agents compile at most three at once | `lock.sh build`, which takes any free one       |

They are `flock` locks on `browser.lock`, `node-N.lock` and `build-N.lock` in
`$WORK/locks`, which the main checkout and all its worktrees share
(`MINA_LOCK_DIR` puts them elsewhere). The directory and the files are made when
first needed. A lock belongs to an open file, so it is released when the process
holding it exits, however it exits; a lock file is never stale, and there is
nothing to clean up after a crash. A slot is not a port: use your agent's ports,
as in [A native bootstrap](#a-native-bootstrap).

- **Waiting:** both wait while what they need is taken, print the holder's line
  (PID, start time, directory and command) whenever it changes, and try again
  every 15 s (`LOCK_POLL`). `LOCK_WAIT=<seconds>` gives up after that long, with
  exit status 75; `LOCK_WAIT=0` doesn't wait.
- **Who has what:** `tools/regression/lock.sh status`.
- **`lock.sh node --log FILE -- COMMAND …`** waits for a free slot, then execs
  the command, which keeps the locked file open, and so the slot, until it
  exits. The command keeps the script's PID: `$!` after `lock.sh … &` is the
  node's PID, and stopping it by PID frees the slot. Messages about the wait go
  to stderr; `--log` sends only the command's output to `FILE`. A process the
  command starts inherits the lock too, and holds it until it exits.
- **`run.mjs`** takes the browser lock before it starts its browser and holds it
  until it exits, after closing the browser; `--dry-run` takes it and lets it go
  at once, or says who has it. Under `lock.sh browser` it uses the lock it
  inherited. The attach scripts take no lock.
- **`MINA_NO_LOCK=1`** skips the locks, for a machine you have to yourself.

Both need `flock` from util-linux. Replays, the rebuild and builds take no lock:
they need only cores and memory (`CARGO_BUILD_JOBS`, the historical baseline).

## A web node in a browser

### Prerequisites

- **Chrome:** `CHROME` is a Chrome or `chrome-headless-shell` binary. On the
  development machine:

  ```bash
  export LD_LIBRARY_PATH=$HOME/.local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu
  export CHROME=$HOME/.local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell
  ```

- **puppeteer-core:** run `npm ci` in `crates/web-verifier/tests/browser`, which
  depends on it, or point `PUPPETEER_DIR` at a directory whose `node_modules`
  has it. In a worktree on the development machine,
  `PUPPETEER_DIR=$ROOT/crates/web-verifier/tests/browser`.

The scripts below need both.

### Building and serving a web node

To test a change to the node, build its WASM and a frontend that points at a
relay. Otherwise use a relay's own build (on the development machine, the test
relay's at `http://localhost:4202`).

[`webnode/build.sh`](webnode/build.sh) does all of the steps below from one
clean commit, and checks that the WASM and the bundle both carry that commit,
which the page shows in its footer and `build-info.json` records with the relay,
genesis and WASM hashes. The build is view-only unless `CONTROLS` names
`producer` or `send` (or `all`), and serves on any port, since its seed list is
a path on the page's own origin. Its start page also offers the block explorer
of the native node at `NATIVE` (default the soak node, `http://127.0.0.1:3200`;
empty for none), which the visitor's browser calls directly:

```bash
RELAY="/dns4/<relay-host>/tcp/<port>/webrtc/https/p2p/<peer-id>" \
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- tools/regression/webnode/build.sh
cd "$WORK/webnode/builds/<commit>-mainnet/browser" &&
    npx -y serve@14 -s . -l $PORT -c serve.json
```

Genesis defaults to the checked-in file for `NETWORK` (mainnet or devnet).
`RELAY` is required; there is no implicit localhost relay. Install the pinned
nightly, its components and the browser using the
[reproduction guide](../../website/docs/developers/reproduce-results.mdx#web-node).

`--help`-style usage is the comment at its top. The steps it runs, as a recipe
(the one in the historical baseline, made explicit; the R26 builds note records
the script's first run, the I8 note what was checked of the recipe before):

1. **WASM,** in `crates/node/web`:

   ```bash
   cargo +nightly-2026-07-15 build --release --target wasm32-unknown-unknown \
       --config 'target.wasm32-unknown-unknown.rustflags=["-C","link-arg=--export=__heap_base"]'
   ```

   Then, from the repo root:

   ```bash
   wasm-bindgen --keep-debug --web --out-dir pkg \
       target/wasm32-unknown-unknown/release/mina_node_web.wasm
   ```

   `build.sh` then names the WASM after its content,
   `mina_node_web_bg.<the first 16 hex digits of its SHA-256>.wasm`, in the one
   place the glue (`mina_node_web.js`) names it, so that a server can let
   browsers keep it (step 4), and writes the encodings a server can send as they
   are next to it: `.br` (brotli quality 11), `.zst` (`zstd -19`) and `.gz`
   (`gzip -9`), about 3.1, 3.4 and 5.7 MB for a 32 MB WASM (R38). It ships the
   verifier index caches of `~/.mina/circuit-blobs` only if they were made from
   the index sources this commit embeds.

2. **Frontend,** in `frontend/` (in a worktree, link `frontend/node_modules` to
   the main checkout's rather than running `npm ci`). `RELAY` is the relay's
   WebRTC address, `/ip4/<ip>/tcp/<http port>/webrtc/http/p2p/<peer id>`, and
   `PORT` the port the build will be served on.

   ```bash
   npx ng build --configuration webnode-local --output-path "$DIST" \
       --define MINA_WEBNODE_NETWORK='"mainnet"' \
       --define MINA_WEBNODE_GENESIS_URL='"/webnode-mainnet/genesis.json"' \
       --define MINA_WEBNODE_BOOTNODES="\"$RELAY\"" \
       --define MINA_WEBNODE_SEED_URLS="\"http://localhost:$PORT/webnode-mainnet/peers.txt\""
   ```

3. **Assemble,** in `$DIST/browser`: replace `assets/webnode/pkg` with the `pkg`
   from step 1; create `webnode-mainnet/` with `genesis.json` (a genesis config
   without ledgers, written by `ledger-tool --without-ledgers`; on the
   development machine `genesis_ledgers/mesa-mainnet-without-ledgers.json`) and
   `peers.txt` (the relay's address); and copy
   [`webnode/serve.json`](webnode/serve.json).

   The backlog also says to copy `webnode.js` to `env.js`. `index.html` loads
   `/environments/env.js`, that is `$DIST/browser/environments/env.js`, into
   `window.env`, so that is the file to replace:
   `cp environments/webnode.js environments/env.js`. It makes no difference to a
   `webnode-local` build, whose settings are compiled in from
   `src/environments/environment.webnode.ts`: the relay's build on 4202 still
   serves the build's placeholder there (`{production: true, configs: []}`; its
   copy went to `assets/environments/env.js`, which nothing loads), and it
   bootstraps.

4. **Serve** it with the headers that make the page cross-origin isolated, which
   the WASM threads need:

   ```bash
   npx -y serve@14 -s . -l $PORT -c serve.json
   ```

   `serve@14` compresses as it sends (brotli for the WASM, about 5.5 MB), and
   `serve.json` lets browsers keep the hash-named WASM for a year (`immutable`)
   while everything else is `no-store`. A public server should send the
   precompressed files instead of compressing 32 MB per request, and keep the
   other files only with revalidation (`no-cache`): the glue, the bundle's
   `index.html`, the seed list and the genesis keep their names from build to
   build. Untested sketches, for Caddy 2.7+:

   ```caddyfile
   webnode.example.org {
   	root * /srv/webnode/browser
   	header Cross-Origin-Opener-Policy "same-origin"
   	header Cross-Origin-Embedder-Policy "require-corp"
   	@wasm path_regexp ^/assets/webnode/pkg/mina_node_web_bg\.[0-9a-f]{16}\.wasm$
   	header @wasm Cache-Control "public, max-age=31536000, immutable"
   	@other not path_regexp ^/assets/webnode/pkg/mina_node_web_bg\.[0-9a-f]{16}\.wasm$
   	header @other Cache-Control "no-cache"
   	try_files {path} /index.html
   	file_server {
   		precompressed br zstd gzip
   	}
   }
   ```

   and nginx (`brotli_static` needs the `ngx_brotli` module; `application/wasm`
   must be in `mime.types`, as it is from nginx 1.21.4):

   ```nginx
   add_header Cross-Origin-Opener-Policy same-origin always;
   add_header Cross-Origin-Embedder-Policy require-corp always;
   gzip_static on;
   brotli_static on;
   location ~ ^/assets/webnode/pkg/mina_node_web_bg\.[0-9a-f]{16}\.wasm$ {
       add_header Cross-Origin-Opener-Policy same-origin always;
       add_header Cross-Origin-Embedder-Policy require-corp always;
       add_header Cache-Control "public, max-age=31536000, immutable" always;
   }
   location / {
       add_header Cross-Origin-Opener-Policy same-origin always;
       add_header Cross-Origin-Embedder-Policy require-corp always;
       add_header Cache-Control no-cache always;
       try_files $uri /index.html;
   }
   ```

   The R9 recipe's relay routes go in the same server block.

### Recording a bootstrap: `run.mjs`

`run.mjs` takes the browser lock ([Taking turns](#taking-turns)), opens the
page, clicks "Start Non BP", and every `EVERY_MS` (30 s) logs the browser's
resident memory, the WASM memory and the node's status. Run it from the
directory it should write to, or set `OUT`:

```bash
mkdir -p /tmp/my-run && cd /tmp/my-run
BASE=http://localhost:4202 UNTIL_SYNCED=60 node "$REPO/tools/regression/webnode/run.mjs" 20
```

It stops 20 minutes after its browser starts, or with `UNTIL_SYNCED=60` a minute
after the node reports `Synced`; time spent waiting for the lock doesn't count.
A bootstrap takes about 10 minutes and 3 GB of memory. `--help` lists its
settings, and `--dry-run` checks them without starting a browser: `CHROME`,
puppeteer-core, that `BASE` serves the page with the COOP and COEP headers, and
whether the browser lock is free. It writes:

- `run.log`: what it prints,
  `<s>s rss=<MB>MB wasm=[<MiB>]MB status={"best_tip":…,"sync":…,"peers":…}`;
- `console.log`: the page's console, each line prefixed with seconds since the
  page was opened;
- `browser.ws`: the browser's DevTools endpoint while it runs;
- `final.png`: a screenshot when it stops.

`PAGE` (`/loading-web-node`) and `BUTTON` (`Start Non BP`) choose the page and
the button. Stopping it with Ctrl-C or `kill <pid>` closes its browser.

`POLL_MS=1000` also calls `status()` from the page every second, one call at a
time and without a timeout, and logs each answer's latency to `console.log` as
`RPCPOLL status <ms> <sync> <best tip> <connected peers>`, so that a slow answer
shows as such rather than as the 10 s timeout of the sampled status. A call
still waiting when the run ends is logged as `RPCPOLL status pending <ms>`. The
I6 note's `analyze.py browser <run dir>` groups the latencies by sync phase,
next to the node's longest events.

`FLOOD=16` runs 16 loops in the page from `FLOOD_FROM` to `FLOOD_TO` seconds
(default 90 and 150) that each call `status()` again as soon as it answers, like
16 HTTP clients polling a native node; with
`FLOOD_FILTER='$.transition_frontier.x'` they call `state().get(FLOOD_FILTER)`,
which serializes that part of the state on the node thread. The page logs
`RPCFLOOD done=<answers> errors=<errors>` every 5 s.

Web nodes bootstrapping through one relay at the same time connect to each other
through it. The browser lock keeps this machine's runs apart, but not web nodes
elsewhere, or runs that ignore it. Before a run meant to measure, check that no
web node is connected to the relay: `curl <relay http>/state/peers` lists them
with `"is_libp2p":false`. Another can still join during the run;
`grep p2p-circuit console.log` shows whether yours connected to one. That is not
the only thing that slows a run, though. Of three runs of this harness, two
shared the relay that way and one had it to itself, and all three took 10.8 to
12.6 minutes against the recorded 9.2, nearly all of the difference in the root
snarked ledger: 205 to 286 s, against 8 s in run 6 (I8 note, and R14).

### Reloads and the frontier store: `reload.mjs`

`reload.mjs` measures the web node's frontier store (I5e, note): the node keeps
its verified ledgers and blocks in the browser's Origin Private File System and
restores from them on a reload. It takes the browser lock and runs one browser
profile throughout, so the store lasts from one page load to the next, through
these phases:

- **bootstrap**: an empty store, a cold start, until synced at the tip, then
  until the first checkpoint is written;
- **reload-N** (`RELOADS`, 3): the page opened anew and the node started again,
  timed to synced at the tip;
- **corrupt**: a byte flipped in the largest stored ledger file, then a start,
  which must fail the store's check and bootstrap;
- with `TAB=1`, a second tab while the first runs, whose store must be locked
  out;
- with `SOAK_MINUTES`, the last reloaded page kept open and the store's size
  read every minute (with a short `CHECKPOINT_SECS`, checkpoints replace each
  other instead of piling up);
- with `QUOTA_MB`, last, an empty store under a quota set through DevTools
  (`Storage.overrideQuotaForOrigin`): the store must be turned off by the full
  quota and the node keep following.

"Synced at the tip" means the node's sync is `Synced` at a best tip at least the
reference node's height minus 1, read from `REFERENCE` (a GraphQL URL) at each
sample:

```bash
BASE=http://localhost:4210 REFERENCE=http://127.0.0.1:3540/graphql \
  OUT=$REPO/.work/<task>/reload node "$REPO/tools/regression/webnode/reload.mjs"
```

It writes `samples.jsonl` (each second: phase, seconds since the page was
opened, sync status, best tip, reference height, WASM memories, browser RSS),
`phases.jsonl` (per phase: time to synced at the tip, peak WASM memory, the
store's files and bytes in OPFS, the node's persistence log lines),
`opfs-<phase>.json`, `console.log` and a screenshot per phase. The page's
localStorage gets `webnodeFrontierStore=on`, and `webnodeCheckpointSecs` with
`CHECKPOINT_SECS`. The comment at the top of the script lists its settings.
Where `npx serve` isn't available, as on the run machine, `serve.py ROOT PORT`
serves a build with the headers it needs.

### Attaching to a running browser

`watch.mjs` (progress every 30 s), `peek.mjs` (evaluate an expression, with
`node` bound to `window.webnode`), `profile.mjs` (CPU-profile every worker) and
`shot.mjs` (screenshot) attach to a browser that is already running:

```bash
export RUN_DIR=/tmp/my-run   # the OUT of your run.mjs
node tools/regression/webnode/peek.mjs 'node.status()'
node tools/regression/webnode/peek.mjs 'node.stats().sync(1)' 2000
node tools/regression/webnode/watch.mjs 10
OUT=/tmp/my-run node tools/regression/webnode/profile.mjs 10 15
node tools/regression/webnode/shot.mjs /tmp/my-run/now.png
```

`RUN_DIR` picks the browser by the `browser.ws` file `run.mjs` wrote. `WS` takes
a DevTools endpoint directly. Without either they attach to the newest puppeteer
browser on the machine, which may be someone else's. `PAGE_MATCH` picks the page
whose URL contains it; the default is the first http(s) page.

Profile once per browser, at the moment that matters. The first `profile.mjs` on
a browser reached all 34 workers and the page; run again on the same browser 2.5
and 5 minutes later, it (and the privately retained original) reached only the
pages, and took 2 minutes, as no worker answered within its 3 s.

## A native bootstrap

The baseline's native row, and the gate for p2p, sync and ledger service
changes, is a native node bootstrapping from empty against mainnet's OCaml
peers, with the genesis config without ledgers. It joins mainnet, so on the
shared machine:

- run it only if your task allows a node that joins mainnet;
- take a node slot with `lock.sh node`, so that there are at most two such extra
  nodes on the machine at once (the historical baseline, and
  [Taking turns](#taking-turns));
- use your agent's ports, HTTP 3400+10N and libp2p 8600+10N for agent N;
- give each run a new, empty data directory, and don't pass `--p2p-secret-key`,
  so that each start gets a new peer id: every node here shares one IP, and
  OCaml's trust system bans by peer (the historical baseline);
- stop it by its PID, never with `pkill` or `killall`.

```bash
N=7                                     # your agent number
HTTP=$((3400 + 10 * N)) P2P=$((8600 + 10 * N))
MINA=$ROOT/target/release/mina          # or your branch's build
RUN=/tmp/native-$(date -u +%H%M) && mkdir -p "$RUN"
# Wait for a free node slot, then start the node, which holds it until it exits.
"$REPO/tools/regression/lock.sh" node --log "$RUN/native.log" -- \
    "$MINA" --network mainnet node -d "$RUN/home" \
    -p $HTTP --libp2p-port $P2P \
    --genesis-config "$WORK/genesis-mainnet/mesa-mainnet-without-ledgers.json" \
    --peer-list-url https://storage.googleapis.com/seed-lists/mainnet_seeds.txt \
    --disable-filesystem-logging --verbosity debug &
PID=$! && echo $PID > "$RUN/pid"
# Wait for Synced, give it a minute, then stop it and wait for it to exit.
until curl -s -m 5 localhost:$HTTP/status | python3 -c 'import json, sys
sys.exit(json.load(sys.stdin)["transition_frontier"]["sync"]["status"] != "Synced")' 2>/dev/null
do kill -0 $PID 2>/dev/null || { echo "it exited early: see $RUN/native.log"; break; }; sleep 10; done
sleep 60; kill $PID 2>/dev/null; while kill -0 $PID 2>/dev/null; do sleep 1; done
tools/regression/baseline.py --replay none --no-reconstruct --native-log "$RUN/native.log"
```

`PID` is the node's from the start, since `lock.sh` execs it, and the slot is
free again once it has exited. While both slots are taken, `lock.sh` says who
has them, and the wait loop keeps waiting. `baseline.py` counts from the log's
first line, which the node writes about half a second after it starts: time
spent waiting for a slot doesn't count, as `--log` only redirects the node's
output.

**This recipe has not been run end to end against mainnet:** neither the task
that wrote it nor its merge was allowed to start a node that joins mainnet. What
was checked:

- the node's flags, against `mina --network mainnet node --help` and the command
  R4 ran;
- its first wait loop, against the soak node's `/status`, with `sleep` standing
  in for the node (I8 note);
- the block above as written, from a worktree, with a stand-in for `mina` that
  writes R4's run 3 log and answers `/status` as `Synced`, with both slots taken
  for its first 6 s and a scratch `MINA_LOCK_DIR`: it waited for a slot, took
  it, wrote only the stand-in's output to `native.log`, had the stand-in's PID
  in `PID`, freed the slot when stopped, and `baseline.py` read 7.2 minutes.

On 2026-10-19 R4 (its note) ran the same node command, without the slot, for
five full bootstraps, with agent 3's ports: run 1 with an older binary, runs 2
to 4 with `main` plus RPC logging, run 5 also with a deeper query. Their logs
are in `$ROOT/logs/r4/run*/node.log`, and `baseline.py` reads them as 6.1, 5.5,
7.2, 8.1 and 4.6 minutes, R4's own totals. R4 found no log of the 13.4-minute
run in the baseline; `$ROOT/logs/native-ledgerless.log`, the 13.5-minute run in
the earlier assessment, reads as 13.6. R4 relates the spread to how many peers
serve ledger queries, so record a few runs rather than one.

## Devnet: a relay and a web node

[`devnet/`](devnet) runs a native node that follows Mesa devnet, keeps it
running, and builds a web node that bootstraps devnet through it. A relay is the
same node with browsers pointed at its HTTP port, which serves WebRTC signaling.
The operator's version, with the ports and numbers, is
[the devnet relay recipe](../../website/docs/node-operators/devnet-relay-and-web-node.md);
the P1f note has the one running on the development machine.

- [`devnet/node.sh`](devnet/node.sh) `RUN_DIR HTTP P2P [KEY]` runs the node in
  the foreground: `--network devnet` (devnet's seeds), the Mesa devnet genesis
  config without ledgers, block proofs verified. With a p2p key file `KEY`
  (made, mode 600, if missing, and passed in `MINA_P2P_SEC_KEY`, never on the
  command line) the peer id, and so the relay address compiled into a web build,
  stays the same across restarts. `HOME` and `TMPDIR` are in `RUN_DIR`, so the
  verifier index cache it writes stays out of `~/.mina`.
- [`devnet/watch.sh`](devnet/watch.sh) is its watchdog, like the soak node's
  `$WORK/soak/watch.sh`: it adopts a node already running (the one in its PID
  file, or the one listening on the node's ports), restarts it from a pinned
  binary only when its process has gone and the ports are free (at most
  `MAX_RESTARTS` times an hour), moves the old log aside on a restart, and every
  minute appends a JSON line from `/status` to a health log: sync status, best
  tip, how far behind its highest peer, connected peers (libp2p and WebRTC),
  resident memory and log size. With `SERVE_DIR` and `SERVE_PORT` it also keeps
  the web build's static server up, never doubling it. One watchdog runs per PID
  file (a lock on `PIDFILE.lock`). `--help` lists its settings;
  [`devnet/test-watch.sh`](devnet/test-watch.sh) tests it with stand-in
  processes in under a minute.
- [`devnet/summary.py`](devnet/summary.py) `HEALTH [EVENTS]` prints the health
  log hour by hour as a Markdown table, then the watchdog's events.
  [`soak_health.py`](soak_health.py) reads the node's log for errors.
- [`devnet/webnode-build.sh`](devnet/webnode-build.sh) `DIST RELAY PORT PKG`
  builds the frontend for devnet with `RELAY` as its only bootnode and seed, the
  devnet genesis config served as `/webnode-devnet/genesis.json`, the WASM in
  `PKG` (a mainnet build's will do: the WASM picks the network at run time) and
  the native node's devnet verifier index cache from `CIRCUIT_CACHE`. Run it
  under `lock.sh build`.

A devnet node doesn't join mainnet, so it takes no node slot; it still shares
the machine's IP with the other nodes. On the development machine (slot 14's
ports):

```bash
RUN=$WORK/devnet/relay KEY=$WORK/devnet/keys/relay.key
# The node, in the background, its log in $ROOT/logs. No setsid: in an
# interactive shell it would fork, and $! would not be the node.
MINA=$ROOT/target/release/mina nohup tools/regression/devnet/node.sh \
    "$RUN" 3540 8740 "$KEY" >> "$ROOT/logs/native-devnet.log" 2>&1 &
echo $! > "$WORK/pids/native-devnet.pid"
# The web build, served on 4224.
RELAY=/ip4/127.0.0.1/tcp/3540/webrtc/http/p2p/$(cat "$RUN/peer-id")
CIRCUIT_CACHE=$RUN/home-env/.mina/circuit-blobs/berkeley-devnet \
    tools/regression/lock.sh build -- tools/regression/devnet/webnode-build.sh \
    "$WORK/devnet/dist-devnet" "$RELAY" 4224 "$PKG"
# A browser bootstrap through it.
BASE=http://localhost:4224 UNTIL_SYNCED=120 OUT=/tmp/my-devnet-run \
    node tools/regression/webnode/run.mjs 30
```

The watchdog takes its settings from the environment; `$WORK/devnet/scripts/` on
the development machine has the copies that run there and `start-watch.sh`,
which sets them.
`python3 tools/regression/devnet/summary.py $ROOT/logs/native-devnet.health`
says how it has been.

## The baseline table

```bash
tools/regression/baseline.py --work "$WORK" --bin-dir "$ROOT/target/release" \
    --browser-run /tmp/my-run
```

It runs the full replay (about 17 minutes) and the rebuild (about 15 s), reads
the bootstrap from `/tmp/my-run`, and prints a markdown table with the numbers
recorded in the historical baseline next to the new ones, then how each was
measured. It also prints the load average at its start and end: on 2026-10-19,
other work on the machine made the replays about 10% and the rebuild 15 to 20%
slower than on a quiet machine.

- `--replay subset` replays only the fast subset; `--replay none` skips it;
  `--replay-log FILE` reads the output of an earlier replay instead (with
  `--replay subset` if it was the subset).
- `--no-reconstruct` skips the rebuild; `--reconstruct DUMP` picks the dump.
- `--browser-console` and `--browser-status` take a run's `console.log` and
  stdout when they are not in one directory, as for runs before this harness.
  Without `--browser-run`, each is read only if given: the memory row needs
  `--browser-status`.
- `--native-log FILE` reads a native node's bootstrap log, as written in
  [A native bootstrap](#a-native-bootstrap).
- `--soak URL` reads a running node's `/status`, and checks for staged ledger
  hash mismatches since the node process started. On a mismatch the node writes
  `failed_application_ctx_<height>.binprot` to its data directory's `debug/` and
  panics, and `$WORK/soak/watch.sh` moves the dump to `$WORK/soak/<height>/` and
  restarts it. `--soak-data-dir`, `--soak-watch-dir` and `--soak-pidfile`
  default to `WORK/mainnet-node`, `WORK/soak` and
  `WORK/pids/native-mainnet-mesa.pid`.
- `--log-dir DIR` keeps the replay and rebuild output there.

`--bin-dir` defaults to this checkout's `target/release`; `--work` is a
directory laid out like `.work/` and sets the default inputs, each of which has
its own flag.

How a bootstrap's phases are read from its log, by the first logged action of
each kind:

| Phase                   | From                                                                     | To                                           |
| ----------------------- | ------------------------------------------------------------------------ | -------------------------------------------- |
| page to synced          | the page opening (native: the first line)                                | `TransitionFrontierSynced`                   |
| staking ledger          | `TransitionFrontierSyncBestTipUpdate`                                    | `TransitionFrontierSyncLedgerStakingSuccess` |
| next epoch ledger       | the staking ledger's end                                                 | `…NextEpochSuccess`                          |
| root ledgers            | the next epoch ledger's end                                              | `…RootSuccess`                               |
| rebuild, within root    | `…StagedReconstructPending`                                              | `…StagedReconstructSuccess`                  |
| blocks                  | the root ledgers' end                                                    | `TransitionFrontierSynced`                   |
| root at                 | the first `LedgerService::block_apply` after the root ledgers, minus one |                                              |
| WASM memory when synced | the first `run.log` sample with `"sync":"Synced"`                        |                                              |

A rebuild starts again when the sync target moves to a new best tip. The rebuild
time counts from the first attempt, as the recorded baseline does; when there
were restarts, the last attempt's time is printed too. Run 6 in the earlier
assessment reads as "rebuild 111 s, last attempt 55 s".

**The chain moves the bootstrap numbers.** The rebuild re-applies the
transactions in the root's scan state, and nearly all its time goes to zkApp
commands, so it varies with how far the root is past the last zkApp-heavy
stretch (the last before 564044 is 563859 to 563909). With the same web node
build, the rebuild took 111 s with the root at 563948 (run 6) and 18 to 23 s at
563988 to 563996; natively, with one binary, it fell from 25 to 15 s as the root
went from 563958 to 563964, and was 9 s at 563975. The root ledgers' and blocks'
phases move with it. Take before and after numbers close together, compare the
printed root heights, and treat a change in the rebuild as the chain's unless
the roots match.

Times in `console.log` are whole seconds, which moves a total by about a second.
Run 6 reads as 557 s, 9.28 minutes, printed as 9.3; the historical baseline
records 9.2, which truncates it or was measured from another start.

## Milestone gate runners

The test, prover, replay and acceptance drivers are in
[`milestone/`](milestone). Paths and binary names are inputs, not the
development machine's checkout or hashed executables. The historical result is
1,025 ordinary/scenario tests in 60 suites; the runner reports measured counts
and never substitutes that expected total for results.

Set absolute paths from your checkout:

```bash
export GATE_DIR="$PWD/gate-results"
export INPUT_DIR="$PWD/reproduction-inputs"
export MINA_BIN="$PWD/target/release/mina"
export MINA_CIRCUIT_BLOBS_BASE_DIR="$INPUT_DIR/circuit-blobs"
export CARGO_BUILD_JOBS=8
bash tools/regression/milestone/build-tests.sh
bash tools/regression/milestone/tests-all.sh
bash tools/regression/milestone/aux-tests.sh
```

- `build-tests.sh` takes the build lock itself, uses the m9 package/features
  selection (libp2p and WebRTC), builds four isolated P2P scenario targets,
  `prove_block` and standalone binprot tests, and checks release workspace
  targets. Cargo JSON messages and metadata go to `GATE_DIR`.
- `pin-executables.py` copies executables and the supplied CLI to `tests/bin/`
  with size/SHA256 receipts, so later builds cannot replace what is tested.
- `run-tests.py MODE [--plan]` preserves the m9 suite filters, skips CLI
  bootstrap tests, records commands, durations, exit codes and libtest counts in
  `tests/MODE/results.jsonl`. `--plan` only prints the selected commands. P2P
  suites use unprivileged user/network namespaces with loopback only; Linux must
  permit `unshare --user --map-root-user --net` and provide `ip`.
- `tests-all.sh` fails at the first failed mode. `summarize-tests.py` writes
  per-mode and per-package counts; focused re-runs are not added to the
  ordinary/scenario total. Use a fresh `GATE_DIR` for a full run.
- `prover-tests.sh` resolves the pinned library executable from Cargo messages
  and runs devnet's 7 and mainnet's 19 tests in separate processes. It checks
  the expected counts and writes `tests/prover/results.json`; neither network's
  blobs are public yet. The generic `ignored` mode excludes these prover tests.
- `aux-tests.sh` runs frontend unit tests, mock watcher tests, the devnet
  watchdog tests and syntax checks. It uses a private temporary directory and
  requires no live node.

The fixture modes (`i5b`, `ignored`, `corpus`) need the rebuild dump, mainnet
and devnet genesis binaries and replay layouts described above. See
[Reproduce our results](../../website/docs/developers/reproduce-results.mdx) for
the publication inventory. Without these inputs, run `ordinary` and the four
`scenario-*` modes individually after pinning; do not claim a full gate.

`full-replay.sh` wraps the offline baseline with explicit inputs and binaries:

```bash
export BIN_DIR="$PWD/target/release"
bash tools/regression/milestone/full-replay.sh --no-reconstruct
```

Omit `--no-reconstruct` only when `INPUT_DIR/reconstruct-current.binprot` is
available. This gate checks staged-ledger hashes, not historical SNARK proofs.

Two opt-in drivers start private test networks; neither is invoked by the test
runner. They need the unpublished OCaml tooling and both prover blob sets.
`run_p46c.py [TARGET=6] [LIMIT=1800]` uses the existing `tools/local-mesa-net`
loopback-only namespace harness, requires `MINA_BIN` and `IDENTITY_BIN` (built
`local_mesa_identity` example), and cleans up through `stop.py`.
`summarize_p46c.py` records OCaml acceptance and timing from that run.
`prover.sh [BLOCKS=6]` drives `tools/webnode-block-prover` under the browser
lock, with explicit `CHROME`, `PKG` (wasm-bindgen web output), `VERIFIER`
(`verify-isolated-proof`), `MINA_BIN` and `LEDGER_TOOL`; it deletes generated
producer keys on exit. Install the browser as described in the reproduction
page. These are opt-in proving experiments; never point them at public networks.

Native bootstrap, follow, persistence and public browser gates use the existing
recipes and tools in this README (`devnet/node.sh`, `webnode/run.mjs`,
`watch-status.py`, the native bootstrap recipe and frontier-store checker). The
old scripts that scheduled specific machine ports, sampled protected services or
seeded local build caches are not prerequisites for these runners.
