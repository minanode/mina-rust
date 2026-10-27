//! P6c, first part: every user command of the Mesa mainnet blocks survives the
//! conversions our pool puts it through, and its signature checks.
//!
//! Our node keeps pooled commands as ledger types and converts them back to
//! wire types to gossip them (`Rebroadcast`, `P2pSend`, the transaction RPC
//! response). OCaml bans, for a day, the peer that sends it a command it can't
//! decode, so the round trip must give back OCaml's bytes exactly. For each
//! command this checks:
//!
//! 1. wire to `UserCommand` to `valid::UserCommand` and back re-encodes to the
//!    same bytes;
//! 2. the pool's hash (`hash_command`) equals the wire hash;
//! 3. a signed command's signature checks with mainnet's prefix, and fails
//!    once its memo is changed.
//!
//! zkApp signatures need the verification keys from the ledger; the replay
//! already checks them (`StagedLedger::check_commands`).
//!
//! It needs blocks that are not in the repo, so it is ignored by default:
//!
//! ```text
//! MINA_REPLAY_DIR=<dir with chain.txt, blocks in it or in blocks/> \
//! [P6C_FROM=<height>] [P6C_TO=<height>] \
//! cargo test -p mina-block-replay --test user_command_corpus -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use ledger::{
    scan_state::transaction_logic::{signed_command::SignedCommand, verifiable, UserCommand},
    transaction_pool::transaction_hash::hash_command,
};
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{MinaBaseSignedCommandMemoStableV1, MinaBaseUserCommandStableV2},
};

fn encode(cmd: &MinaBaseUserCommandStableV2) -> Vec<u8> {
    let mut bytes = Vec::new();
    cmd.binprot_write(&mut bytes).unwrap();
    bytes
}

fn env_height(name: &str) -> Option<u32> {
    std::env::var(name).ok().map(|v| v.parse().unwrap())
}

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR"]
fn user_commands_round_trip_hash_and_verify() {
    let _ = mina_core::NetworkConfig::init("mainnet");
    let dir = PathBuf::from(std::env::var("MINA_REPLAY_DIR").expect("MINA_REPLAY_DIR"));
    let blocks = match dir.join("blocks") {
        blocks if blocks.is_dir() => blocks,
        _ => dir.clone(),
    };
    let from = env_height("P6C_FROM").unwrap_or(0);
    let to = env_height("P6C_TO").unwrap_or(u32::MAX);

    let chain = std::fs::read_to_string(dir.join("chain.txt")).unwrap();
    let (mut block_count, mut signed, mut zkapps) = (0, 0, 0);
    for line in chain.lines() {
        let mut parts = line.split_whitespace();
        let (Some(height), Some(hash)) = (parts.next(), parts.next()) else {
            continue;
        };
        let height: u32 = height.parse().unwrap();
        if height < from || height > to {
            continue;
        }
        let block = mina_block_replay::read_block(&blocks, height, hash)
            .unwrap_or_else(|e| panic!("{height} {hash}: {e:#}"));
        block_count += 1;

        let diff = &block.staged_ledger_diff.diff;
        let commands = diff
            .0
            .commands
            .iter()
            .chain(diff.1.iter().flat_map(|d| d.commands.iter()))
            .map(|c| &c.data);
        for (i, wire) in commands.enumerate() {
            let at = format!("block {height}, command {i}");
            let bytes = encode(wire);

            let command = UserCommand::try_from(wire).unwrap_or_else(|e| panic!("{at}: {e}"));
            let valid = command.clone().to_valid_unsafe();
            let back: MinaBaseUserCommandStableV2 = (&valid.forget_check()).into();
            assert!(encode(&back) == bytes, "{at}: re-encoding differs");

            let pooled = hash_command(command.clone().to_valid_unsafe());
            assert_eq!(pooled.hash, wire.hash().unwrap(), "{at}: hash");

            match wire {
                MinaBaseUserCommandStableV2::SignedCommand(signed_wire) => {
                    signed += 1;
                    let cmd = SignedCommand::try_from(signed_wire).unwrap();
                    assert!(cmd.check_valid_keys(), "{at}: keys");
                    assert!(
                        verifiable::check_only_for_signature(Box::new(cmd)).is_ok(),
                        "{at}: signature"
                    );

                    let mut mutated = signed_wire.clone();
                    let mut memo = mutated.payload.common.memo.0.as_ref().to_vec();
                    memo[2] ^= 1;
                    mutated.payload.common.memo = MinaBaseSignedCommandMemoStableV1(memo.into());
                    let mutated = SignedCommand::try_from(&mutated).unwrap();
                    assert!(
                        verifiable::check_only_for_signature(Box::new(mutated)).is_err(),
                        "{at}: a changed memo must not verify"
                    );
                }
                MinaBaseUserCommandStableV2::ZkappCommand(_) => zkapps += 1,
            }
        }
    }
    println!(
        "{block_count} blocks: {signed} signed commands and {zkapps} zkApp commands round-trip \
         byte-identically and hash as on the wire; every signed command verifies"
    );
    assert!(block_count > 0, "no blocks in range");
}
