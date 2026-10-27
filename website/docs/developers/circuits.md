---
sidebar_position: 4
title: Circuit Generation and Management
description: Circuit logic implementation and proof generation capabilities
slug: /developers/circuits
---

# Circuit Generation and Management

## Overview

The Mina Rust node has ported the circuit logic from the Mina protocol, but with
an important architectural distinction: the implementation only handles witness
production, not constraint generation. This means that while the Mina Rust node
can produce proofs using existing circuits, it cannot generate the circuit
definitions themselves.

For an overview of the proof system implementation in `ledger/src/proofs/`, see
the
[ledger crate documentation](https://o1-labs.github.io/mina-rust/api-docs/ledger/index.html).

## Architecture

### Proof Generation Implementation and Limitations

The Mina Rust node codebase includes complete proof generation capabilities with
one key limitation:

**What the Mina Rust node Can Do:**

- **Witness generation**: Full implementation for producing witnesses needed for
  proof generation
- **Proof production**: Complete capability to create proofs using pre-existing
  circuit definitions
- **Circuit logic**: Equivalent to the OCaml implementation for all proof types
- **Proof verification**: Can verify proofs using precomputed verification
  indices

**What the Mina Rust node Cannot Do:**

- **Circuit constraints**: Missing the constraint declarations from the OCaml
  code that define circuit structure
- **Constraint compilation/evaluation**: Missing the functionality to
  compile/evaluate constraint declarations into circuit constraints
- **Verification key generation**: Cannot generate verification keys for new
  circuits

**Practical Implications:**

- Can generate proofs and witnesses for existing circuits
- Cannot create new circuits or modify existing circuit definitions
- Relies on OCaml implementation for all circuit creation and constraint
  processing
- Uses precomputed verification indices from the OCaml implementation

The circuit logic is equivalent to the OCaml implementation except both the
constraint declarations and the constraint compilation/evaluation functionality
are missing - these were not ported due to time constraints during development,
not technical limitations, and could be added for full independence.

### Circuit Generation Process

Since these constraint capabilities are missing, the Mina Rust node requires
externally generated circuit data. The following process describes how circuits
are created and distributed using the original Mina codebase:

1. **Circuit Definition**: Circuits are defined using the OCaml implementation's
   constraint system
2. **Index Generation**: Verification and proving indices are generated from the
   circuit definitions
3. **Distribution**: Pre-generated indices are distributed for use by Rust nodes
4. **Proof Generation**: The Mina Rust node uses these indices to generate and
   verify proofs

## Implementation Details

### Witness Production

The Mina Rust node implements complete witness production for all supported
proof types:

- **Transaction proofs**: Witness generation for user command verification
- **Block proofs**: Witness production for blockchain state transitions
- **Merge proofs**: Witness generation for proof aggregation
- **Base proofs**: Witness production for foundational protocol operations

### Proof Types Supported

#### Transaction Proofs

- User command verification
- Payment and delegation transactions
- zkApp account updates and state changes

#### Blockchain Proofs

- Block state transition verification
- Consensus state updates
- Protocol state evolution

#### SNARK Work Proofs

- Transaction SNARK generation
- Proof merging and aggregation
- Work verification

### Circuit Data Management

#### Verification Indices

- Pre-computed verification keys from OCaml implementation
- Distributed as binary data files
- Loaded at runtime for proof verification

#### Proving Indices

- Pre-computed proving keys for proof generation
- Large binary files stored separately
- Lazy-loaded when proof generation is required

## Performance Characteristics

### Witness Generation

- **Speed**: Comparable to OCaml implementation
- **Memory Usage**: Efficient memory management during witness production
- **Parallelization**: Some witness generation can be parallelized

### Proof Production

- **Throughput**: Supports concurrent proof generation
- **Resource Usage**: CPU and memory intensive operations
- **Optimization**: Optimized for production workloads

## Integration with Protocol

### Block Producer Integration

- Seamless integration with block production pipeline
- Automatic proof generation for produced blocks
- Efficient witness caching and reuse

### Transaction Pool Integration

- On-demand proof generation for transactions
- Batch processing for multiple transactions
- Memory-efficient proof storage

### Archive Integration

- Proof verification for historical blocks
- Efficient storage of verification results
- Support for proof re-verification

## Limitations and Future Work

### Current Limitations

#### Constraint System

- Missing constraint declaration framework
- No support for custom circuit creation
- Dependent on OCaml implementation for new circuits

#### Verification Key Generation

- Cannot generate verification keys independently
- Requires external tooling for circuit updates
- Limited flexibility for protocol upgrades

### Future Improvements

#### Constraint Implementation

- **Goal**: Port constraint declaration system from OCaml
- **Benefit**: Full independence from OCaml implementation
- **Effort**: Significant development work required

#### Circuit Optimization

- **Goal**: Rust-specific circuit optimizations
- **Benefit**: Improved performance over OCaml version
- **Effort**: Moderate development work

#### Custom Circuit Support

- **Goal**: Enable creation of custom circuits
- **Benefit**: Support for protocol evolution and experimentation
- **Effort**: Requires constraint system implementation first

## Development Guidelines

### Working with Circuits

#### Adding New Proof Types

1. Implement witness generation logic
2. Define proof type structure
3. Add integration points with existing systems
4. Test with precomputed verification indices

#### Optimizing Performance

1. Profile witness generation bottlenecks
2. Optimize memory allocation patterns
3. Consider parallelization opportunities
4. Benchmark against OCaml implementation

#### Debugging Circuit Issues

1. Use structured logging for witness generation
2. Compare outputs with OCaml reference implementation
3. Validate proof generation against known test vectors
4. Monitor memory usage during proof production

### Testing Strategy

#### Unit Tests

- Individual witness generation functions
- Proof type serialization and deserialization
- Circuit data loading and validation

#### Integration Tests

- End-to-end proof generation and verification
- Performance benchmarks against OCaml
- Memory usage validation

#### Compatibility Tests

- Cross-verification with OCaml-generated proofs
- Protocol compliance validation
- Regression testing for circuit changes

### Circuit Generation Process

Since these constraint capabilities are missing, the Mina rust nodes require
externally generated circuit data. The following process describes how circuits
are created and distributed using the original Mina codebase:

:::warning Work in Progress

This should be updated when a release of the OCaml node happens that contains
code to export circuits, and when command to export circuits is added, and CI is
updated to check for latest circuits

:::

1. Build mina <b>OCaml</b> node from source with commit after
   [`6961849`](https://github.com/MinaProtocol/mina/commit/6961849f17d564c39e7d45e01e3ddda9a09602a4)

2. Running the circuit generation process using the branch above
   - Launch the OCaml node which produces circuit cache data in
     `/tmp/coda_cache_dir`
   - The branch dumps the usual circuit data plus extra data specifically
     required by the Mina rust nodes, see
     [Circuit somponents](#circuit-components)
   - The process also dumps blocks for use in tests, see
     [Testing strategy](#testing-strategy)
   - Integration with mainline Mina would streamline future circuit generation

3. The generated circuit blobs are then:
   - Committed to the dedicated repository:
     https://github.com/o1-labs/circuit-blobs
   - Released as GitHub releases for versioning and distribution

### Mesa circuits

No circuit-blobs release has the Mesa (OCaml 4.0.2) mainnet circuits, which
mainnet's `CircuitsConfig` names in the `4.0.2mainnet` directory. The official
4.0.2 binary dumps them offline, in about a minute, with no source build:

```bash
tools/mesa-circuits/generate.sh /tmp/mesa-blobs
export MINA_CIRCUIT_BLOBS_BASE_DIR=/tmp/mesa-blobs/circuit-blobs
```

The script runs the binary with `MINA_DUMP_CIRCUIT_DATA=1`, checks that it wrote
all 8 circuits and that the blockchain wrap key is the live one, and links the
24 files into `circuit-blobs/4.0.2mainnet`. The output is deterministic. The
prover looks there first, then in the crate directory, `~/.mina/circuit-blobs`
and `/usr/local/lib/mina/circuit-blobs`, before trying a release.

With the blobs in place, the Mesa prover tests, ignored by default, prove and
verify one real mainnet base job of each non-zkApp kind: a payment, a
delegation, a failed payment and a failed delegation, fee transfers to one and
two receivers, and coinbases with and without a fee transfer. Each proof must
verify with the mainnet transaction verifier and fail with another sok digest:

```bash
cargo test --release -p mina-tree --lib -- --ignored proofs::transaction::mesa_tests
```

The same tests merge proofs with the merge circuit: OCaml's two base proofs of
the first merge job on Mesa mainnet, OCaml's two merge proofs of the first merge
one level up, one of those OCaml base proofs next to a Rust one, and two Rust
base proofs. Each merge must verify with the mainnet transaction verifier, and
fail with another sok digest or a tampered statement.

The fixtures, `crates/ledger/src/proofs/data/mesa_mainnet_*_base_job.bin`, are
inputs a snark worker gets, not proofs. The merge fixtures,
`mesa_mainnet_merge_of_*_job.bin`, hold the two OCaml proofs to merge. Two
examples of `mina-block-replay` write both kinds from a replay of Mesa mainnet:
`mesa_base_job_fixtures` and `mesa_merge_job_fixtures`.

zkApp commands are proved in segments, one or two account updates each, with the
three zkApp step circuits (`opt_signed-opt_signed`, `opt_signed` and `proved`),
and the segments' proofs merged into one. Their ignored tests prove real Mesa
zkApp command base jobs this way: mainnet ones with signature pairs, a proof and
a single signature (block 548185), with two proofs in a row (548186, a token
mint) and with signatures only (548945). Each segment's witness must satisfy its
circuit, a changed signature or target ledger must not, and the merged proof
must have the statement, sok digest included, of OCaml's snark work for the job,
verify with the mainnet transaction verifier, and fail with another sok digest
or the devnet key:

```bash
cargo test --release -p mina-tree --lib -- --ignored --test-threads=1 \
  proofs::zkapp::mesa_tests --skip mesa_devnet
```

Their fixtures, `mesa_mainnet_zkapp_*_base_job.bin` with OCaml's work for each
in `mesa_mainnet_zkapp_*_ocaml_work.bin`, come from the
`mesa_zkapp_job_fixtures` example of `mina-block-replay`, which also replays
devnet from a staged ledger dump. `zkapp_prove_from_env` and
`zkapp_segments_constraints_from_env` prove, or only check against the circuits,
any job written by it (`MINA_ZKAPP_JOB`, `MINA_ZKAPP_WORK`,
`MINA_ZKAPP_NETWORK`); `MINA_WITNESS_TRACE_AT` prints the code that pushed given
witness values, as named in the failing rows.

The block circuits have their own ignored tests, on the base case: the genesis
transition of Mesa mainnet, as the node proves its genesis block. The block step
witness must have the Mesa `AUX_LEN` and satisfy the circuit, and fail it when
the ledger statement changes without a ledger proof (the constraint Mesa added).
The genesis block must prove, step and wrap, and verify with the mainnet
blockchain verifier, the live key, for the genesis state only:

```bash
cargo test --release -p mina-tree --lib -- --ignored proofs::block::mesa_tests
```

Their fixture, `mesa_mainnet_genesis_block_input.bin`, is the block prover's
input, written by the `mesa_genesis_block_input` example from the prebuilt
genesis config. The `prove_block` example proves such an input outside the tests
and writes the state and proof in OCaml's JSON (a proof is the base64 of its
S-expression); with `--verify` it checks a block in that JSON, such as one that
OCaml's `internal test-genesis-block-generation` printed, with the mainnet
verifier.

The same tests prove blocks after genesis, on the previous block's proof. A real
Mesa mainnet block can't be proved again: the block step evaluates the VRF in
the circuit with the producer's private key. So the `mesa_offline_chain` example
produces a chain whose genesis is Mesa mainnet's but for the staking epoch
ledger, where the genesis winner, whose private key is public, holds most of the
stake. Each block carries the user commands of the real block at its height and
is built by the node's block producer code, then proved and checked with the
mainnet verifier:

```bash
cargo run --release -p mina-block-replay --example mesa_offline_chain -- \
  --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
  --count 30 --out offline-chain
```

It writes each block's prover input to `offline-chain/inputs`, and the blocks
with their proofs to `offline-chain/blocks.json`, which `prove_block --verify`
checks. Two of the inputs are fixtures, `mesa_offline_chain_548148_input.bin`
and `mesa_offline_chain_548149_input.bin`: their witnesses must satisfy the
block step, and the second block's proof must verify for its state only. With
`--repeat <n>`, `prove_block` proves an input n times in one process and keeps
kimchi's error or the rejected proof of every failed attempt: a stress test for
intermittent block proof failures.

A real transition can still be checked against the block step, all but its VRF.
The `mesa_real_transition_check` example rebuilds the block prover's input of a
real Mesa mainnet block from the precomputed blocks and a replay, with OCaml's
proof of the parent and the ledger proof the block emits, puts the genesis
winner's key in place of the producer's, and lists every constraint the witness
fails: only those that depend on the key must fail. The fixture
`mesa_mainnet_548415_block_input.bin`, the first Mesa block with a ledger proof,
pins the seven checks it fails.

The node checks its own block proofs the same way before broadcasting a block,
and proves the block a second time if the prover fails or the proof does not
verify. After two failures it drops the block and discards the won slot
(`BlockProducerAction::BlockProveError`), which it does not take again. With
proof verification off (`--proof-level none`) it proves once, without the check.
The check is what keeps a proof that peers would reject off the network: a
broken copy constraint, such as a block proved on the wrong previous proof,
fails every time, so the second attempt only helps with a failure that does not
follow from the input.

### Mesa devnet circuits

Devnet has its own set, in the `4.0.2devnet` directory that devnet's
`CircuitsConfig` names. The same script dumps it with the devnet profile and the
Mesa devnet fork config:

```bash
MINA_PROFILE=devnet tools/mesa-circuits/generate.sh /tmp/mesa-devnet-blobs \
  tools/mesa-circuits/mesa-devnet-fork.json
export MINA_CIRCUIT_BLOBS_BASE_DIR=/tmp/mesa-devnet-blobs/circuit-blobs
```

The 24 files must have the hashes in `tools/mesa-circuits/devnet-blobs.sha256`,
from the dump made with the OCaml 4.0.2 binary (commit `84b8d969`):

```bash
(cd /tmp/mesa-devnet-blobs/circuit-blobs/4.0.2devnet &&
  sha256sum --check --strict --quiet "$OLDPWD/tools/mesa-circuits/devnet-blobs.sha256")
```

Use another output directory than mainnet's; to have both sets under one base
directory, link `4.0.2mainnet` and `4.0.2devnet` into one `circuit-blobs`
directory. The devnet circuits have the mainnet circuits' gates, wiring and
sizes; seven of the eight differ in a few constants (the testnet signature
prefix, devnet's fork constants, and the step keys the wrap circuits embed), and
the merge circuit is mainnet's. So a proof made with one set verifies with that
network's key only. The devnet indexes built from the blobs match the embedded
devnet keys, the live blockchain key and OCaml's transaction key, field by
field:

```bash
cargo run --release -p mina-tree --example mesa_circuit_blobs -- --network devnet \
  /tmp/mesa-devnet-blobs/tmp/coda_cache_dir data/vk/devnet-mesa-blockchain-vk.json
```

The devnet prover tests are in the same modules and run with the devnet blobs,
one network per test process, as the network config is global:

```bash
cargo test --release -p mina-tree --lib -- --ignored --test-threads=1 mesa_devnet
```

They prove a real devnet payment, merges of OCaml's and Rust's devnet base
proofs, zkApp commands (one with a recursive zkApp's proof, whose key has wrap
domain 2^14, and two signed ones sent through our node and our web node, blocks
577969 and 578035), and blocks of an offline chain from the Mesa devnet genesis
(`mesa_offline_chain --network devnet`), whose block 545436 carries a real
devnet payment. Each proof must verify with the devnet key and fail with the
mainnet key, and the mainnet tests check the converse. The examples
`mesa_offline_chain`, `prove_block` and `mesa_real_transition_check` take
`--network devnet`.

### Circuit Components

Each circuit consists of multiple components that are loaded and cached
independently:

- **Gates**: Circuit constraint definitions in JSON format (`*_gates.json`)
- **Internal Variables**: Constraint variable mappings in binary format
  (`*_internal_vars.bin`)
- **Rows Reverse**: Row-wise constraint data in binary format (`*_rows_rev.bin`)
- **Verifier Indices**: Pre-computed verification data with SHA256 integrity
  checks

### Overview

Circuit constraints for the Mina Rust node are sourced from the
[circuit-blobs](https://github.com/o1-labs/circuit-blobs) repository, which
contains pre-compiled circuit data generated by the OCaml implementation.

The Mina Rust node automatically handles downloading and caching these circuit
files, making the process transparent to users. When you run the node or
generate proofs, the system will automatically fetch the required circuit data
if it's not already available locally.

## Related Documentation

- [Proof System Overview](https://o1-labs.github.io/mina-rust/api-docs/ledger/proofs/index.html):
  Technical implementation details
- [SNARK Work](../researchers/snark-work): Protocol-level SNARK work
  documentation
- [Architecture Overview](architecture): Overall system architecture
- [Performance Considerations](mainnet-readiness): Mainnet performance
  requirements

## Conclusion

While the Mina Rust node successfully implements witness production and proof
generation, the missing constraint system represents a significant dependency on
the OCaml implementation. This architectural choice was made to accelerate
initial development but represents an area for future enhancement to achieve
full protocol independence.

The current implementation provides sufficient functionality for mainnet
operation while maintaining compatibility with the broader Mina ecosystem.
Future work on constraint system implementation would enable full circuit
independence and support for protocol evolution.
