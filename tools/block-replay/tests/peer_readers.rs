//! The checks the bin_prot readers make on what peers send (nesting depth,
//! signers that decompress, token symbol and string lengths, length
//! prefixes) accept every block of the replay chain. Each precomputed block,
//! with the accounts it touched, is written as bin_prot, read back, and must
//! write the same bytes again; the block also reads back as gossip, and its
//! commands and accounts convert to the ledger's types.
//!
//! It needs the replay blocks, so it is ignored by default:
//!
//! ```text
//! MINA_REPLAY_DIR=<dir with chain.txt, blocks in it or in blocks/> \
//! cargo test --release -p mina-block-replay --test peer_readers -- --ignored --nocapture
//! ```
//!
//! `MINA_REPLAY_TO=<height>` stops at that height.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
};

use ledger::{scan_state::transaction_logic::UserCommand, Account};
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    gossip::{decode_pubsub_data, GossipNetMessageV2},
    v2::PrecomputedBlockData,
};
use rayon::prelude::*;

fn encode<T: BinProtWrite>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// `value` written, read back, and written again to the same bytes.
fn round_trip<T: BinProtRead + BinProtWrite + PartialEq + std::fmt::Debug>(value: &T) {
    let bytes = encode(value);
    let read = T::binprot_read(&mut &bytes[..]).unwrap();
    assert_eq!(&read, value);
    assert_eq!(encode(&read), bytes);
}

#[test]
#[ignore = "needs the replay blocks: MINA_REPLAY_DIR"]
fn chain_blocks_pass_the_peer_readers() {
    let dir = PathBuf::from(std::env::var("MINA_REPLAY_DIR").expect("MINA_REPLAY_DIR"));
    let blocks = match dir.join("blocks") {
        blocks if blocks.is_dir() => blocks,
        _ => dir.clone(),
    };
    let to = std::env::var("MINA_REPLAY_TO")
        .ok()
        .map(|height| height.parse::<u32>().unwrap());
    let chain: Vec<(u32, String)> = std::fs::read_to_string(dir.join("chain.txt"))
        .unwrap()
        .lines()
        .map(|line| {
            let (height, hash) = line.split_once(' ').unwrap();
            (height.parse().unwrap(), hash.to_owned())
        })
        .filter(|(height, _)| to.is_none_or(|to| *height <= to))
        .collect();

    let (commands, accounts) = (AtomicUsize::new(0), AtomicUsize::new(0));
    chain.par_iter().for_each(|(height, hash)| {
        let bytes = mina_block_replay::read_block_bytes(&blocks, *height, hash).unwrap();
        let data = PrecomputedBlockData::from_ocaml_json(&bytes)
            .unwrap_or_else(|e| panic!("{height} {hash}: {e}"));
        round_trip(&data);

        let block = Arc::new(data.data.block());
        let message = GossipNetMessageV2::NewState(block.clone());
        let body = encode(&message);
        let gossip = [(body.len() as u64).to_le_bytes().to_vec(), body].concat();
        assert_eq!(decode_pubsub_data(&gossip).unwrap(), message, "{height}");

        let diff = &block.body.staged_ledger_diff.diff;
        let second = diff.1.iter().flat_map(|d| d.commands.iter());
        for command in diff.0.commands.iter().chain(second) {
            round_trip(&command.data);
            UserCommand::try_from(&command.data)
                .unwrap_or_else(|e| panic!("{height}: command: {e:?}"));
            commands.fetch_add(1, Relaxed);
        }
        for (_, account) in data.data.accounts_accessed.iter() {
            let account =
                Account::try_from(account).unwrap_or_else(|e| panic!("{height}: account: {e:?}"));
            let _ = account.hash();
            accounts.fetch_add(1, Relaxed);
        }
    });
    eprintln!(
        "{} blocks, {} commands, {} accounts accessed: all read back",
        chain.len(),
        commands.into_inner(),
        accounts.into_inner()
    );
}
