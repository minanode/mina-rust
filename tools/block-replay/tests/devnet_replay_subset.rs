//! Devnet replays, each checking every staged ledger hash:
//!
//! - `devnet_genesis_head`: Mesa devnet from the P1c genesis
//!   (`mesa-devnet.bin`) to 545441, the chain's first 7 blocks;
//! - `devnet_replay_subset`: from a devnet node's root at 577343 to 577630, 287
//!   blocks: 3 to 4 s of replay after about 50 s rebuilding the root's staged
//!   ledger.
//!
//! The public bucket no longer has the first Mesa devnet blocks: every devnet
//! block below 552017 was deleted on 2026-10-20. Of the 6,582 heights it lost,
//! only the first 7 blocks were kept here (by R13), so a replay from genesis
//! stops at 545441 and the subset starts from a root dump instead
//!.
//!
//! They need data that is not in the repo, so they are ignored by default:
//!
//! ```text
//! MINA_DEVNET_REPLAY_DIR=<dir with chain.txt, the root dump, the blocks, genesis-head/> \
//! MINA_DEVNET_GENESIS_BIN=<mesa-devnet.bin> \
//! cargo test --release -p mina-block-replay --test devnet_replay_subset -- --ignored --nocapture
//! ```
//!
//! `tools/regression/README.md` says how to get them, and how long they take.

mod common;

use std::time::Duration;

use common::{Start, Subset};

/// Mesa devnet's genesis, height 545434, which `mesa-devnet.bin` must give.
const GENESIS: Start = Start::Genesis {
    bin_var: "MINA_DEVNET_GENESIS_BIN",
    height: 545434,
    hash: "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ",
};

const GENESIS_HEAD: Subset = Subset {
    network: "devnet",
    replay_dir_var: "MINA_DEVNET_REPLAY_DIR",
    replay_subdir: "genesis-head",
    start: GENESIS,
    to: (
        545441,
        "3NL1cBAJHEDHFhUah8CHYxkVoQNENjCJgJMkHkkv7Q1zHva7GBG2",
    ),
    budget: Duration::from_secs(60),
};

const FROM_ROOT: Subset = Subset {
    network: "devnet",
    replay_dir_var: "MINA_DEVNET_REPLAY_DIR",
    replay_subdir: "",
    // The root of a devnet node that bootstrapped on 2026-10-21 with
    // `MINA_DUMP_RECONSTRUCT` set.
    start: Start::Dump {
        height: 577343,
        hash: "3NK5JHqJydM4Js9mzk6euWvsUHcgMuttG4KL6xtiXsf2jmnwwpyG",
    },
    to: (
        577630,
        "3NK3gpoZnZHV1qv8wk19eaZrrEE7b4GuuT3W52rNdeaqKRWkrZw3",
    ),
    budget: Duration::from_secs(180),
};

#[test]
#[ignore = "needs devnet replay data: set MINA_DEVNET_REPLAY_DIR and MINA_DEVNET_GENESIS_BIN, and use --release"]
fn devnet_genesis_head() {
    common::replay(&GENESIS_HEAD);
}

#[test]
#[ignore = "needs devnet replay data: set MINA_DEVNET_REPLAY_DIR, and use --release"]
fn devnet_replay_subset() {
    common::replay(&FROM_ROOT);
}
