# Protocol-state fixtures

Each JSON file contains a parent and child protocol state, with their published
state hashes, from the public `mina_network_block_data` precomputed-block
bucket. The tests verify both hashes before checking transitions. These are
captured OCaml states, not output from the Rust state builder.

- `mainnet-548149.json`: early seed-update window, no emitted proof.
- `mainnet-551056.json`: outside the seed-update window.
- `mainnet-557000.json`: later epoch, inside the seed-update window.
- `mainnet-564336.json`: the canonical block with a timestamp 78.208 seconds
  into its 90-second slot, cited in
  [MinaProtocol/mina#19901](https://github.com/MinaProtocol/mina/pull/19901).

The source URL for each entry is
`https://storage.googleapis.com/mina_network_block_data/mainnet-HEIGHT-STATE_HASH.json`,
where `HEIGHT` is its `blockchain_length` and `STATE_HASH` its `state_hash`.
Ledger-free tests use the original child's staged hash and proof statement as
fixed inputs. Full block replay independently derives them by applying the
staged-ledger diff.
