//! The fast replay subset: Mesa mainnet from genesis to height 551260, 3,113
//! blocks, checking every staged ledger hash. It ends inside the chain's first
//! zkApp-heavy stretch, 551243 to 551296: its last 18 blocks hold 213 zkApp
//! commands with 6,603 account updates. The rest of the stretch repeats the
//! same kind of command, and replaying it too goes over 3 minutes once the
//! machine is busy. Earlier blocks are mostly payments, with two stretches of
//! over 5,000 signed commands per 250 blocks. `replay_from_dump` starts later
//! on the chain instead, from a staged ledger dump.
//!
//! It needs data that is not in the repo, so it is ignored by default:
//!
//! ```text
//! MINA_REPLAY_DIR=<dir with chain.txt, blocks in it or in blocks/> \
//! MINA_GENESIS_BIN=<mesa-mainnet.bin> \
//! cargo test --release -p mina-block-replay --test replay_subset -- --ignored --nocapture
//! ```
//!
//! `tools/regression/README.md` says how to get both, and how long it takes.
//! `devnet_replay_subset.rs` is the same for devnet.

mod common;

use std::time::Duration;

use common::{Start, Subset};

const MAINNET: Subset = Subset {
    network: "mainnet",
    replay_dir_var: "MINA_REPLAY_DIR",
    replay_subdir: "",
    // Mesa mainnet's genesis: the subset is every block after it to `to`.
    start: Start::Genesis {
        bin_var: "MINA_GENESIS_BIN",
        height: 548147,
        hash: "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7",
    },
    // Keep in step with `SUBSET_TO` in `tools/regression/baseline.py`.
    to: (
        551260,
        "3NKAGGsspiboutUBQxoJ8ibVfMJszsYuWz7TWAqTeGaceohbqGcC",
    ),
    budget: Duration::from_secs(180),
};

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR and MINA_GENESIS_BIN, and use --release"]
fn replay_subset() {
    common::replay(&MAINNET);
}
