# Fixtures for `verify_every_block`

Precomputed mainnet blocks from the public `mina_network_block_data` bucket,
zstd-compressed, named `<height>-<state hash>.json.zst` as in the replay's
directory.

| File                                                                   | Chain    | Completed work         | Source                                             |
| ---------------------------------------------------------------------- | -------- | ---------------------- | -------------------------------------------------- |
| `548148-3NKPoFh4wB6MvvJPTLdfnL3GUNnCB3pLG2AC35cF2pcKfue3cG9s.json.zst` | Berkeley | none                   | R1's `mainnet-548148-3NKPoFh4….json`, recompressed |
| `550855-3NKm1wv7kCB1fnWLXNU9THuHuSgpXD7rf4C83jKGvNao7UfdMY9k.json.zst` | Mesa     | one work of two proofs | OCaml replay capture, as fetched                   |
| `551056-3NKYoVGoKeJAi1mQ2yE8p934o4sh7KGJ8oohDDHsNCAMfDhnuZzh.json.zst` | Mesa     | one work of one proof  | OCaml replay capture, as fetched                   |
| `548215-3NLRHYJ1ZAwZf741aTqUzvRfFrGQZ47anzGttnkyTvbnFw39Kri5.json.zst` | Mesa     | none; one zkApp proof  | OCaml replay capture, as fetched                   |

The Berkeley block carries the Berkeley genesis `3NK4BpDS…` and protocol version
3.0.0: it is a block of the old chain at the height of the first Mesa blocks
(private implementation notes). The two Mesa blocks are the smallest of the
2,266 Mesa mainnet blocks to 564044 that carry completed work. The first Mesa
block, 548148 `3NKtYZVH…`, comes from
`crates/web-verifier/tests/fixtures/mainnet-548148.json`.

548215 holds one applied zkApp command whose second account update sets a
verification key (hash `0x27153A44…`) and whose third is authorized by a proof
against it, so the block carries both the key and the proof. Of the 37 Mesa
mainnet blocks to 564044 with proof-authorized account updates, it is one of the
four that set the key they use.

## Devnet

For `verify_devnet_blocks`, from P1e's privately retained devnet chain (the
public bucket's blocks, as fetched):

| File                                                                          | What                                                                                         |
| ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `devnet-570120-3NLWtuFrYuet6NvY4TxBWbHCuEm7L6oHkVU2auGGqVzoDakR18Er.json.zst` | Mesa devnet block, no completed work; one zkApp proof against key `0x1972ADF1…`, not set     |
| `devnet-569564-verification-key.json`                                         | that key, `{ "data", "hash" }` as OCaml wrote it in 569564 `3NKPLuuH…`, the block setting it |

570120 is the smallest of the ten blocks from 570119 to 570128, the first to use
the key, 556 heights after 569564 sets it. The two other devnet files are
trimmed blocks for `precondition_amounts` (see that test).
