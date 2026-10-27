# Real Mesa mainnet SNARK work

Completed works from precomputed Mesa mainnet blocks, each a binprot
`TransactionSnarkWorkTStableV2` (fee, proofs, prover), as a block's staged
ledger diff and the snark pool carry them. The tests in `../snark_work.rs`
verify them against the embedded mainnet transaction verifier key and check that
tampered copies fail, so a key or verifier regression fails `cargo test`.

| File                    | Block                                                         | Work | Proofs (wrap branch) | Fee (nanomina) | Prover                                                    |
| ----------------------- | ------------------------------------------------------------- | ---- | -------------------- | -------------- | --------------------------------------------------------- |
| `mainnet-551267-1.bin`  | 551267 `3NLVJabkWq61auZZhTTSZVh9By8ab48UhQb1Whgjz27F1fJwZbxx` | 1    | `N0`, `N0` (base)    | 9,495,000      | `B62qrB4hLHkwUz3UXwx6jLx6XrvbRae4d8t6pMVaGhjt2c1XoqJZTUb` |
| `mainnet-556667-40.bin` | 556667 `3NLh2F7zMZ6MaBCAck7U6GWqNAvTtz1GamtrXJtGeJGEKW7PaUJj` | 40   | `N2` (merge)         | 10,000,000     | `B62qmFMKE2uti9wcEfR5nNAr49x6QeeV2nhZwHAYsFXr53GDqT6faNo` |
| `mainnet-548252-46.bin` | 548252 `3NKK7ouABUbnUF1iNf3KoJQpC1jMNFnJwPFcPk2L4rxkMSi1frrD` | 46   | `N2`, `N0`           | 0              | `B62qopzjbycAJDzvhc1tEuYSmJYfRQQbfS9nvkKtUzBS1fmLCyTz4dJ` |

Works are numbered as the block lists them, the first diff part's before the
second's. The blocks come from the public `mina_network_block_data` bucket
(`tools/block-replay/fetch_chain.py`). The files were written with:

```bash
cargo run --release -p mina-block-replay --example snark_work_fixtures -- \
    --chain "$CHAIN_FILE" --blocks "$BLOCKS_DIR" \
    --pick 551267:1 --pick 556667:40 --pick 548252:46 \
    --out crates/ledger/src/verifier/fixtures
```

`--list HEIGHT` prints a block's works, with their wrap branches and sizes, to
choose others.
