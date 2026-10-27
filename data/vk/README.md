# Mesa blockchain verification keys

These are the live Pickles blockchain-SNARK verification keys, in the format the
GraphQL `blockchainVerificationKey` query returns.

| File                              | Network        | Source                                                                                                                                    | sha256           |
| --------------------------------- | -------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ---------------- |
| `mainnet-mesa-blockchain-vk.json` | mainnet (Mesa) | `api.minascan.io/node/mainnet/v1/graphql`; node on `4.0.0-mainnet-mesa` (6850301), SYNCED, 2026-10-18                                     | `37b6abdf…79c45` |
| `devnet-mesa-blockchain-vk.json`  | devnet (Mesa)  | a local isolated `mina daemon` 4.0.2 (84b8d96), `MINA_PROFILE=devnet`, with `mina-devnet-config_4.0.2` `config_84b8d969.json`, 2026-10-18 | `6471ab08…47ad7` |

**Do not use the golden files in the OCaml repo**
(`src/lib/blockchain_snark/tests/print_blockchain_snark_vk/*_blockchain_snark_vk.json`).
They match the live keys in every commitment except `coefficients_comm`, which
bakes in network-specific constants, so blocks fail to verify against them.

Public endpoints are unreliable for devnet: `api.minascan.io`'s devnet node runs
3.0.4-alpha2, a pre-Mesa build stuck in BOOTSTRAP, and returns the old Berkeley
key. Run a local daemon to get the devnet key (see the earlier assessment).

## Verification results (2026-10-18)

Run natively with `docs/probes/verify_precomputed_file.rs`, built as an example
inside a worktree of `MinaProtocol/mina-verify@release/mesa` (7d12c95). That
checkout uses proof-systems ab84160 and dkijania/mina-rust@49cc4ea. Blocks come
from the public `mina_network_block_data` bucket.

| Block                                               | Key         | Result                                                                                           |
| --------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------ |
| mainnet 552000, 560000                              | mainnet     | **true**, about 140 ms warm                                                                      |
| mainnet 560000 with `blockchain_length + 1`         | mainnet     | false (tamper rejected)                                                                          |
| devnet 545450 … 570000 (11 heights)                 | devnet      | **true**, about 115–145 ms warm                                                                  |
| mainnet 560000                                      | devnet      | false (cross-network rejected)                                                                   |
| devnet 570000                                       | mainnet     | false (cross-network rejected)                                                                   |
| mainnet 548147; devnet 545434, 545440 (`3NK53BJM…`) | own network | false, as it should be: these are blocks of the old Berkeley chain, not Mesa blocks (see below). |

**Blocks right after the fork (2026-10-19).** The fork point is the Berkeley
block named in the Mesa config (`proof.fork`): mainnet 548146 `3NKHyxzg…`,
devnet 545433 `3NLYmfj4…`. The Mesa genesis is its child (mainnet 548147
`3NLntCRR…`, devnet 545434 `3NLT7n4L…`), and the bucket has no file for it. The
first Mesa blocks are at 548148 and 545435. The old chain also ran on for hours
after the fork point, so the public bucket holds blocks of both chains at the
same heights: on mainnet 57 old-chain blocks at 548147 to 548187, on devnet
old-chain blocks up to 545493. The blocks above that failed are all from the old
chain: protocol version 3.0.0, Berkeley genesis state hash. They verify with the
Berkeley key on proof-systems 0.3.0. Every Mesa block tried right after the fork
verifies with the Mesa key: 14 mainnet blocks from 548148 and 11 devnet blocks
from 545435. Select blocks by parent links or genesis state hash, not by height.
`crates/web-verifier/tests/mesa.rs` verifies a first Mesa block on each network
(mainnet 548148, devnet 545435) and rejects the old-chain devnet 545434.
