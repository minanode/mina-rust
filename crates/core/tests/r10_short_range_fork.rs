//! R10: the short-range fork rule at the end of the seed update range, and
//! which epochs it compares.
//!
//! OCaml calls an older chain's next epoch data final when the slot *after*
//! its tip is outside the seed update range, so a tip at slot 4759 of 7140
//! already counts. Before I26 the Rust check asked for the tip's own slot to
//! be outside the range (>= 4760), and compared `epoch_count` where OCaml
//! compares the epochs of the tips' slots.

use mina_core::consensus::is_short_range_fork;
use mina_p2p_messages::v2::{
    ConsensusProofOfStakeDataConsensusStateValueStableV2 as ConsensusState,
    MinaNumbersGlobalSlotSinceHardForkMStableV1, UnsignedExtendedUInt32StableV1,
};

/// The consensus state of mainnet block 564253, as `/state` serves it.
const BLOCK_564253: &str = r#"{
    "blockchain_length": "564253",
    "epoch_count": "6",
    "min_window_density": "13",
    "sub_window_densities": ["1", "5", "2", "4", "1", "2", "1", "3", "1", "2", "0"],
    "last_vrf_output": "UBHn0LW8007t5i_Zfe1TG2PskA06sm0ru-ZjsxYSAAA=",
    "total_currency": "1307274188840039233",
    "curr_global_slot_since_hard_fork": {"slot_number": "44311", "slots_per_epoch": "7140"},
    "global_slot_since_genesis": "1002751",
    "staking_epoch_data": {
        "ledger": {
            "hash": "jxRhCwcDDFbb49ZP3p5YppH9YtXkw5d2vpYzDnnx1v6cB35AdQR",
            "total_currency": "1306203242840039233"
        },
        "seed": "2vbsQR2KSF8TPNC9sLnLjksxX981f3zc1QoVtarUYyT9Ni8PhhCS",
        "start_checkpoint": "3NK42A41Zyk352Ydvoz4B6eQ4YPB9TSMa3cc4WSymTywt3mwZSyx",
        "lock_checkpoint": "3NKjGYVxAR6Kx1HYQKFUVJgMTLmcr8eHxvK1dZJ4CjqZDdXyhuE1",
        "epoch_length": "2251"
    },
    "next_epoch_data": {
        "ledger": {
            "hash": "jxJp66JhmUbFo4U9rYiELCHQ2hgXzuHwYJffh1yY4Lp93hfH1X4",
            "total_currency": "1306935639840039233"
        },
        "seed": "2vatDSmPgpY7GGTzfGMrXWDPGZwVAHvBgHi9f9aVsdQzgxapgdt8",
        "start_checkpoint": "3NLNsJ6jvz7RkQNuqPJUbynJQTvLhmqbTb7PPrHxpBX1d9rzS1Tn",
        "lock_checkpoint": "3NKrPgNZDrKzp7xe4R5FpLnrADGwKLT7DZDJ5t1NsKT14wfDXWBH",
        "epoch_length": "458"
    },
    "has_ancestor_in_same_checkpoint_window": true,
    "block_stake_winner": "B62qqBLrVQjF4EVxKFungW5TSoPWFvZqDwZyRXe3c8v1JbB1UJ9pb4T",
    "block_creator": "B62qq3TQ8AP7MFYPVtMx5tZGF3kWLJukfwG1A1RGvaBW1jfTPTkDBW6",
    "coinbase_receiver": "B62qiX9eUkBF6LnUrAzsrPB1hakcaoQjk2uuqkd4wYykDp471BcyP7E",
    "supercharge_coinbase": true
}"#;

const SLOTS_PER_EPOCH: u32 = 7140;

fn at(epoch: u32, slot_in_epoch: u32) -> ConsensusState {
    let mut state: ConsensusState = serde_json::from_str(BLOCK_564253).unwrap();
    state.epoch_count = UnsignedExtendedUInt32StableV1::from(epoch);
    state.curr_global_slot_since_hard_fork.slot_number =
        MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
            (epoch * SLOTS_PER_EPOCH + slot_in_epoch).into(),
        );
    state
}

/// An epoch 6 tip at `slot_in_epoch`, and an epoch 7 tip on the same history:
/// its staking lock checkpoint is the older tip's next lock checkpoint.
fn pair(slot_in_epoch: u32) -> (ConsensusState, ConsensusState) {
    let older = at(6, slot_in_epoch);
    let mut newer = at(7, 10);
    newer.staking_epoch_data.lock_checkpoint = older.next_epoch_data.lock_checkpoint.clone();
    (older, newer)
}

#[test]
fn older_tip_past_the_seed_update_range() {
    let (older, newer) = pair(4760);
    assert!(is_short_range_fork(&older, &newer));
    assert!(is_short_range_fork(&newer, &older));
}

/// The slot where `is_short_range_fork` and OCaml's `is_short_range` differed.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2771-L2787>
#[test]
fn older_tip_at_the_last_seed_update_slot() {
    // OCaml: not (in_seed_update_range (4759 + 1)) holds, so the next epoch
    // data are final and the lock checkpoints decide: a short-range fork.
    let (older, newer) = pair(4759);
    assert!(is_short_range_fork(&older, &newer));
    assert!(is_short_range_fork(&newer, &older));
}

#[test]
fn older_tip_inside_the_seed_update_range() {
    let (older, newer) = pair(4758);
    assert!(!is_short_range_fork(&older, &newer));
    assert!(!is_short_range_fork(&newer, &older));
}

/// The newer chain had an epoch without blocks, so its `epoch_count` is one
/// less than its epoch. OCaml compares `curr_epoch`, from the slot: the tips
/// are in consecutive epochs, so the older tip's final next epoch data decide.
#[test]
fn epochs_come_from_slots_not_epoch_count() {
    let (older, mut newer) = pair(4760);
    newer.epoch_count = older.epoch_count;
    assert!(is_short_range_fork(&older, &newer));
    assert!(is_short_range_fork(&newer, &older));
}

/// `epoch_count`s one apart, but epochs two apart (5 and 7): not the
/// previous-epoch case, whatever the lock checkpoints.
#[test]
fn epochs_two_apart_are_a_long_range_fork() {
    let older = at(5, 4760);
    let mut newer = at(7, 10);
    newer.epoch_count = UnsignedExtendedUInt32StableV1::from(6);
    newer.staking_epoch_data.lock_checkpoint = older.next_epoch_data.lock_checkpoint.clone();
    assert!(!is_short_range_fork(&older, &newer));
    assert!(!is_short_range_fork(&newer, &older));
}
