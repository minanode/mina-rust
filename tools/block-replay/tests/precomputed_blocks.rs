//! Precomputed blocks against the public bucket's, which OCaml nodes write:
//!
//! - `round_trip_every_block` reads every held block and writes it back, which
//!   must give the same bytes;
//! - `node_output_matches` applies the replay subset from genesis as the node
//!   does and builds the precomputed block the node's archive service writes
//!   for each block, which must give the bucket's bytes, `scheduled_time`
//!   aside: OCaml and the node write when they write the block.
//!
//! They need data that is not in the repo, so they are ignored by default:
//!
//! ```text
//! MINA_REPLAY_DIR=<dir with chain.txt, blocks in it or in blocks/> \
//! MINA_GENESIS_BIN=<mesa-mainnet.bin> \
//! cargo test --release -p mina-block-replay --test precomputed_blocks -- --ignored --nocapture
//! ```
//!
//! `MINA_PRECOMPUTED_TO` sets the last height of `node_output_matches`
//! (default 551260, the replay subset's end).

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::Once,
};

use mina_block_replay::{
    compare::{self, Replay, Summary},
    read_block_bytes,
};
use mina_node::ledger::write::LedgerHashes;
use mina_p2p_messages::v2::PrecomputedBlockData;
use rayon::prelude::*;

fn init_network() {
    static INIT: Once = Once::new();
    INIT.call_once(|| mina_core::NetworkConfig::init("mainnet").unwrap());
}

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name}; see tools/regression/README.md"))
}

fn chain(to: Option<u32>) -> (PathBuf, Vec<(u32, String)>) {
    let dir = env_path("MINA_REPLAY_DIR");
    let blocks = match dir.join("blocks") {
        blocks if blocks.is_dir() => blocks,
        _ => dir.clone(),
    };
    let file = std::fs::File::open(dir.join("chain.txt")).expect("chain.txt");
    let chain = BufReader::new(file)
        .lines()
        .map(|line| line.expect("chain.txt"))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.to_owned()))
        })
        .take_while(|(height, _)| to.is_none_or(|to| *height <= to))
        .collect();
    (blocks, chain)
}

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR, and use --release"]
fn round_trip_every_block() {
    init_network();
    let (blocks, chain) = chain(None);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    let summary = pool.install(|| {
        chain
            .par_iter()
            .fold(Summary::default, |mut summary, (height, hash)| {
                let result = read_block_bytes(&blocks, *height, hash)
                    .and_then(|bytes| compare::round_trip(&bytes));
                summary.add(&format!("{height}-{hash}"), result);
                summary
            })
            .reduce(Summary::default, |mut a, b| {
                a.merge(b);
                a
            })
    });
    println!("{summary}");
    assert!(summary.is_clean(), "{summary}");
}

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR and MINA_GENESIS_BIN, and use --release"]
fn node_output_matches() {
    if cfg!(debug_assertions) {
        panic!("run with --release: an unoptimised replay takes hours");
    }
    init_network();
    let to = std::env::var("MINA_PRECOMPUTED_TO")
        .ok()
        .map(|to| to.parse().expect("MINA_PRECOMPUTED_TO"))
        .unwrap_or(551260);
    let (blocks, chain) = chain(Some(to));
    let mut replay = Replay::from_genesis(&env_path("MINA_GENESIS_BIN")).unwrap();
    let mut summary = Summary::default();
    for (height, hash) in &chain {
        let name = format!("{height}-{hash}");
        let bytes = read_block_bytes(&blocks, *height, hash).unwrap();
        let bucket = compare::read(&bytes).unwrap();
        let ours = replay
            .apply(&bucket.data)
            .unwrap_or_else(|e| panic!("block {name}: {e:#}"));
        let result = PrecomputedBlockData::from(ours)
            .to_ocaml_json(&LedgerHashes)
            .map_err(anyhow::Error::from)
            .and_then(|ours| compare::compare(&bytes, &ours));
        summary.add(&name, result);
    }
    println!("{summary}");
    assert_eq!(summary.blocks, chain.len());
    assert!(summary.is_clean(), "{summary}");
}
