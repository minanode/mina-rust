# Updating mina-rust at a hard fork

This is the release procedure for the next Mina hard fork, worked through with
Mesa and OCaml 4.0.2 (`84b8d96954b218f372b93f65a071abf528e6bb61`). The node runs
one fork per process. Updating configuration does not port new consensus rules,
regenerate circuits, or replace verification keys. Ship those together as one
release, independently for mainnet and devnet.

## Before the old chain stops

1. Record the official release commit, proof-systems pin, runtime configuration,
   network profile, scheduled stop slots and target genesis time. Keep the old
   release, its keys and its data separately so the old chain remains auditable.
2. Ship the old-chain stop configuration before its transaction stop. Both
   `daemon.slot_tx_end` and `daemon.slot_chain_end` are **inclusive slots since
   the current hard fork**, not slots since the original genesis:
   - before `slot_tx_end`, normal transactions, coinbases and work are allowed;
   - at and after `slot_tx_end`, the transaction pool rejects new or backtracked
     transactions. Producers emit completely empty staged ledger diffs: no user
     commands, completed work, coinbase, internal statuses or second pre-diff.
     Validators reject any non-empty diff. Existing queued transactions cannot
     make it into those blocks;
   - at and after `slot_chain_end`, producers stop and validators reject every
     block. A chain stop without a transaction stop still stops blocks;
   - absent stop slots impose no deadline. Neither setting automatically exits
     the Rust process or writes the next genesis configuration.
3. At the agreed final old-chain block, obtain the new runtime config and
   ledgers. OCaml can generate them with
   `mina advanced generate-hardfork-config` or
   `--hardfork-handling migrate-exit`; inspect that release's help before using
   either. `hard_fork_genesis_slot_delta` schedules OCaml's migration; mina-rust
   does not implement that migration controller.

The reference rules are
[validator.ml, L42–106](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_handler/validator.ml#L42-L106),
[block_producer.ml, L177–237](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L177-L237),
[indexed_pool.ml, L901–917](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/indexed_pool.ml#L901-L917)
and
[diff.ml, L512–524](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/staged_ledger_diff/diff.ml#L512-L524).
OCaml constructs the new fork config in
[runtime_config.ml, L1708–1743](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L1708-L1743).

## Configuration and release assets

`--config daemon.json` reads the OCaml-shaped runtime config. `--genesis-config`
reads either a compact JSON genesis or a prebuilt binary; the web node uses the
same loader. Compact JSON carries top-level `proof` and `daemon` alongside its
existing ledger hashes, currencies, seeds and protocol constants. New binaries
written by `PrebuiltGenesisConfig::store` start with `MINAGEN1`, a four-byte
little-endian metadata length, JSON runtime metadata, then the former bin_prot
payload. Legacy binaries and JSON without metadata retain compiled defaults.
Regenerate bundles from the runtime config; an old bundle cannot recover values
that were discarded when it was written. Older Rust releases cannot read the new
binary header.

The runtime proof fields match OCaml's
[make_constraint_constants](https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/lib/genesis_ledger_helper_lib.ml#L416-L482):

- `fork.state_hash`: the final old-chain block's state hash;
- `fork.blockchain_length`: that block's height;
- `fork.global_slot_since_genesis`: the **new genesis slot**, which need not be
  the final old block's slot;
- `sub_windows_per_window`, `ledger_depth`, `work_delay`,
  `block_window_duration_ms`, `supercharged_coinbase_factor`;
- `coinbase_amount` and `account_creation_fee`: decimal MINA strings;
- `transaction_capacity`: exactly one of `2_to_the` (also `two_to_the` or
  `log_2`) and `txns_per_second_x10`. OCaml derives pending coinbase depth from
  capacity and work delay; there is no independent runtime depth override.

Missing values fall back to `crates/core/src/network.rs`. Initialization happens
before constructing consensus/ledger services; conflicting initialization in the
same process fails. Restart to select another fork. A minimal Mesa mainnet proof
section is:

```json
{
  "proof": {
    "fork": {
      "state_hash": "3NKHyxzgM9z4C1wUz5ZhJBmNhtDzDQGr1PGvWRwiCUcFvsdTco2T",
      "blockchain_length": 548146,
      "global_slot_since_genesis": 958440
    },
    "block_window_duration_ms": 90000,
    "coinbase_amount": "360"
  }
}
```

This is a fragment, not a complete genesis config. Keep `genesis.*` protocol
constants and timestamp, genesis/staking/next ledgers and seeds with it. Stop
slots belong to the chain being stopped; do not copy old-chain deadlines into
the new config without translating their intended meaning.

Audit these code and asset locations for each release:

- `crates/core/src/network.rs`: fallback profiles, signatures, circuit digests,
  gate/cache names and seeds; `constants.rs`: protocol versions and defaults.
- `crates/node/src/daemon_json/` and `transition_frontier/genesis/`: runtime
  parsing, genesis construction and every export/import format.
- `crates/core/src/consensus.rs`, `block/`, `crates/ledger/src/scan_state/` and
  `staged_ledger/`: rule changes. Runtime parsing is not a promise that every
  historical fixed assumption can vary. In particular, audit fixed ledger depths
  and fork-choice sub-window/grace constants before changing those parameters;
  the latter have a separate R10 follow-up.
- `crates/core/src/block/prevalidate.rs`, the node ledger service, producer
  guards and `crates/ledger/src/transaction_pool.rs`: stop-slot enforcement.
- `crates/p2p-messages/src/v2/`, RPC menus and codecs: OCaml stable-type
  changes.
- `crates/ledger/src/proofs/data/`, `proofs/verifiers.rs`, circuit blobs and
  prover witnesses: both verification and production assets.
- Embedded genesis data in `crates/node/src/config.rs`, web genesis bundles,
  relay settings, seeds and frontend network labels: release them together.

`daemon` currently supplies the pool size and stop slots, not OCaml's entire set
of fee/zkApp limits. Its peer-list URL is preserved, but startup still uses the
CLI `--peer-list-url` option. The chain-id reducer still uses the compiled
pool-size input. If a fork changes those limits, port and test the consumers as
well; do not rely on an otherwise ignored JSON field.

## Read values from OCaml, not from Rust output

Use the new release's source and binaries. On the shared machine, repositories
and ignored inputs live in a separately configured checkout; always reference
those inputs by absolute path. Give every OCaml invocation a private `TMPDIR`.
For Mesa:

```sh
ROOT=$(git rev-parse --show-toplevel)
OCAML_CONFIG=/path/to/ocaml/config_84b8d969.json
mkdir -p /tmp/p11/ocaml-mainnet
TMPDIR=/tmp/p11/ocaml-mainnet MINA_PROFILE=mainnet \
  scripts/ocaml-mina advanced compile-time-constants
CARGO_BUILD_JOBS=8 TMPDIR=/tmp/p11/ocaml-mainnet \
  "$ROOT/tools/regression/lock.sh" build -- \
  scripts/ocaml-chain-id-trace.sh mainnet \
  "$OCAML_CONFIG"
```

Repeat the trace for devnet with its packaged configuration and a different
private temporary directory. The trace script and
`scripts/ocaml-chain-id-split.py` expose the six circuit digests **in chain-id
order**, the zero-currency genesis state hash, the genesis constants hash and
version hashes. `advanced constraint-system-digests` compiles the blockchain
step without the runtime fork: its blockchain-step digest is not the forked
chain's digest. The signed transaction and zkApp digests also depend on the
signature kind.

Regenerate P11d's independent mainnet fixture, saving a new file before
reviewing the diff:

```sh
CARGO_BUILD_JOBS=8 TMPDIR=/tmp/p11/ocaml-mainnet \
  "$ROOT/tools/regression/lock.sh" build -- \
  python3 -I scripts/ocaml-constants-fixture.py \
  --config "$OCAML_CONFIG" \
  --source-dir "$ROOT/repos/mina" --out /tmp/p11/new-constants.json
```

The source directory must match the binary's commit. The script is currently
mainnet-specific; setting `MINA_PROFILE=devnet` alone does **not** convert it
into a devnet checker. Adapt the profile/source readers before using it for
devnet or another release. Optional `--block` and `--graphql` add independent
observations. Compare all observations, update pinned expectations only from
OCaml evidence, and record provenance. See P11d (private qualification notes).

## Regenerate keys and prover circuits

1. Obtain the blockchain verification key from a confirmed new-chain OCaml node
   (`blockchainVerificationKey`) or generate it offline with the exact runtime
   fork config. Record the node version, chain ID and full key hash. The golden
   OCaml blockchain keys can have different `coefficients_comm`; they are not
   automatically the live fork's keys. See
   [key provenance](../data/vk/README.md).
2. Convert that key with the `export_verifier_index` example and replace the
   appropriate embedded blockchain index. For the transaction key, export from
   the matching transaction circuit or use the OCaml golden transaction key only
   after checking it against live transaction proofs. P1 used
   `src/lib/transaction_snark/test/print_transaction_snark_vk/devnet_transaction_snark_vk.json`.
   Regenerate caches; never interpret cache presence as evidence of key parity.

   For example, build the converter under the lock, then run it on the
   independently obtained public key outside the lock:

   ```sh
   CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR="$ROOT/target" \
     "$ROOT/tools/regression/lock.sh" build -- \
     cargo build --release -p mina-web-verifier --example export_verifier_index
   "$ROOT/target/release/examples/export_verifier_index" \
     data/vk/mainnet-mesa-blockchain-vk.json /tmp/p11/blockchain-index.json
   ```

   Review the output against
   `crates/ledger/src/proofs/data/mainnet_blockchain_verifier_index.json` (or
   the corresponding network/transaction index) before replacing that asset.

3. Dump all eight step/wrap circuits and their 24 files with
   `tools/mesa-circuits/generate.sh <out> <fork-config.json>`, separately per
   network. Use `MINA_PROFILE=devnet` for devnet. The script makes `<out>/tmp`
   its private `TMPDIR`, so place the output beneath `/tmp/p11` on this machine.
   Circuit compilation also takes the build lock, for example
   `CARGO_BUILD_JOBS=8 "$ROOT/tools/regression/lock.sh" build -- tools/mesa-circuits/generate.sh /tmp/p11/mainnet-circuits tools/mesa-circuits/mesa-mainnet-fork.json`.
   Run `generate.sh --check <out>` and the `mina-tree` example
   `mesa_circuit_blobs`. Match indexes positionally against OCaml, not merely
   the presence of commitment coordinates.
4. Set the release-specific `CIRCUITS_CONFIG` names and
   `MINA_CIRCUIT_BLOBS_BASE_DIR`. Port witness changes, row/AUX sizes and
   proof-systems behavior. The script is Mesa-specific: review its release and
   live-key assumptions before reusing it for a later fork. Never reuse a
   previous release's directory just because a gate filename stayed the same.
5. Verify real blockchain and transaction proofs, including base, merge and
   zkApp jobs, and reject tampered and wrong-network proofs. Also prove captured
   jobs and a full block with Rust and verify them with the new OCaml node.
   Following a chain does not prove block-production compatibility.

## Deal with overlapping chains

Block height is not a chain identifier. Preserve distinct old/new storage,
configurations, keys and peer connections. Start the new network with its new
chain ID/pnet key; do not try to continue an old frontier under new constants.

Mesa mainnet's fork point was height 548146, genesis 548147 and first produced
Mesa block 548148. Devnet's were 545433, 545434 and 545435. Genesis is computed,
not a published bucket block. The bucket's file at each genesis height was an
**old Berkeley-chain block**, and both chains continued at overlapping heights.
The devnet final old block's slot was 859399, but the new genesis slot
was 859560. These facts initially looked like verification failures near the
fork. R1 and R13 resolved them by checking genesis hashes and parent links.

Fetch replay inputs by walking parents back from a verified new-chain tip
(`tools/block-replay/fetch_chain.py`), or filter by genesis state hash and check
parent continuity. Retain old-chain fixtures as negative verification tests. Do
not pick an arbitrary file at each height. Release new relay and browser bundles
together so their keys, genesis and chain ID agree.

## Release gates and Mesa worked example

The coordinator integrates and merges; implementation agents do not push to or
merge `main`. On the development machine every compile runs under the absolute
`tools/regression/lock.sh build --` path with `CARGO_BUILD_JOBS=8`. Compile
tests with `--no-run`, then execute their binaries outside that lock.

1. Run the `mina-core`, `mina-node` and `mina-tree` suites. `mina-tree --lib`
   must have no failures. Check the P11d constants suites, both chain-id tests,
   `devnet_mesa_genesis`, `runtime_mainnet_genesis`, `runtime_fork_config` and
   `stop_slots`. Record any unrelated known failures separately; never call a
   failing suite green.
2. Recompute the real and zero-currency genesis hashes from the new
   configuration. Test full daemon JSON, compact JSON and prebuilt data. Confirm
   defaults only apply when fields are absent, and conflicting initialization
   fails.
3. Verify fork-boundary fixtures in `mina-web-verifier`, including rejection of
   Berkeley fixtures by Mesa keys. Verify every blockchain proof and every
   completed SNARK work item in the chosen chain, using the project's current
   `verify_every_block`/`verify_snark_work` tools when integrated.
4. Run the full staged-ledger replay, not just the fast subset. Set
   `GENESIS_FILE`, `CHAIN_FILE` and `BLOCKS_DIR` to your verified inputs. Mesa's
   baseline:

   ```sh
   mina-block-replay --network mainnet \
     --genesis "$GENESIS_FILE" \
     --chain "$CHAIN_FILE" \
     --blocks "$BLOCKS_DIR"
   ```

   Require all 15,897 staged-ledger hashes through 564044 to match OCaml. For a
   new fork, construct and record an equivalent parent-linked chain starting at
   its first produced block. Replay devnet independently, including custom-token
   accounts.

5. Briefly start a native node on assigned ports with the new genesis, check its
   computed chain ID and a real peer connection, then run the authorized sync,
   epoch-boundary and soak checks. Extra mainnet nodes require `lock.sh node`;
   browser runs require `tools/regression/webnode/run.mjs`. Never attach to or
   restart the shared live nodes. Stop only your own PIDs and retain logs.
6. Before enabling production, prove and submit a devnet block accepted by
   OCaml. Schedule the network switch and keep a rollback plan that restores the
   old release/config/data together, never mixed assets.

Mesa's implementation history supplies concrete precedents:

- `63b04341`: 90 s slots and 360 MINA coinbase for mainnet;
- `ce5a8520`, `8c6efa6b`: chain-id inputs and the fork-aware blockchain digest;
- `342cf3d0`, `6dc9d696`, `1213c731`: devnet constants/digests, verifier indexes
  and corrected seed ports (P1);
- `0b30341c`, `5c3f3a2b`, `c843c9cc`: circuit extraction, Mesa blob layouts and
  three prover fixes (P4.1/P4.2);
- P11d: the independent constants fixture and per-block zkApp hardcap;
- R10 (private qualification notes), R13 (private qualification notes), P1
  (private qualification notes) and P11 results (private qualification notes):
  provenance, checks and remaining limitations.

The unchanged Mesa chain IDs are
`0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886` (mainnet) and
`ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8` (devnet).
They are evidence for these exact releases and configs, not constants to carry
forward into the next fork.
