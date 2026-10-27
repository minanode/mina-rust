use mina_p2p_messages::v2::{
    self, BlockTimeTimeStableV1,
    ConsensusProofOfStakeDataConsensusStateValueStableV2 as MinaConsensusState, StateHash,
};
use redux::Timestamp;
use serde::{Deserialize, Serialize};
use time::{macros::format_description, OffsetDateTime};

use crate::constants::constraint_constants;
pub use crate::constants::{
    checkpoint_window_size_in_slots, slots_per_window, CHECKPOINTS_PER_YEAR,
};

// TODO get constants from elsewhere
const SUB_WINDOWS_PER_WINDOW: u32 = 11;
const SLOTS_PER_SUB_WINDOW: u32 = 7;
const GRACE_PERIOD_SLOTS: u32 = 2160;
/// `grace_period_slots + slots_per_window`, in slots since the hard fork.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/constants.ml#L239-L240>
const GRACE_PERIOD_END: u32 = GRACE_PERIOD_SLOTS + SUB_WINDOWS_PER_WINDOW * SLOTS_PER_SUB_WINDOW;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ConsensusShortRangeForkDecisionReason {
    ChainLength,
    Vrf,
    StateHash,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ConsensusLongRangeForkDecisionReason {
    SubWindowDensity,
    ChainLength,
    Vrf,
    StateHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusTime {
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub epoch: u32,
    pub global_slot: u32,
    pub slot: u32,
}

/// The epoch of a state's slot since the hard fork, and the slot within it
/// (`Consensus_state.curr_epoch_and_slot`).
fn curr_epoch_and_slot(s: &MinaConsensusState) -> (u32, u32) {
    // Never 0 in a valid state; `max` only avoids a division by zero.
    let slots_per_epoch = s
        .curr_global_slot_since_hard_fork
        .slots_per_epoch
        .as_u32()
        .max(1);
    let slot = s.global_slot();
    (slot / slots_per_epoch, slot % slots_per_epoch)
}

/// Whether two chains are a short-range fork of each other: whether their
/// most recent finalized checkpoints are equal.
///
/// The epochs are those of the tips' slots, not `epoch_count`: they differ
/// after an epoch without blocks.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2771-L2787>
// TODO(binier): do we need to verify constants? Probably they are verified
// using block proof verification, but check just to be sure.
pub fn is_short_range_fork(a: &MinaConsensusState, b: &MinaConsensusState) -> bool {
    // `c1` is one epoch behind `c2`, and its next epoch data are final, so
    // they are `c2`'s staking epoch data. They are final once the slot after
    // `c1`'s tip is past the seed update range: from slot 4759 of 7140, as
    // that slot's successor, 4760, is outside it.
    let pred_case = |c1: &MinaConsensusState, c2: &MinaConsensusState| {
        let (e1, slot1) = curr_epoch_and_slot(c1);
        let (e2, _) = curr_epoch_and_slot(c2);
        let slots_per_epoch = c1.curr_global_slot_since_hard_fork.slots_per_epoch.as_u32();
        let in_seed_update_range = slot1.saturating_add(1) < slots_per_epoch / 3 * 2;
        crate::log::trace!(crate::log::system_time(); kind = "is_short_range_fork", msg = format!("epochs {e1} and {e2}, slot {slot1}, c1 next lock checkpoint {}, c2 staking lock checkpoint {}", c1.next_epoch_data.lock_checkpoint, c2.staking_epoch_data.lock_checkpoint));
        e1.checked_add(1) == Some(e2)
            && !in_seed_update_range
            && c1.next_epoch_data.lock_checkpoint == c2.staking_epoch_data.lock_checkpoint
    };

    let (a_epoch, _) = curr_epoch_and_slot(a);
    let (b_epoch, _) = curr_epoch_and_slot(b);
    if a_epoch == b_epoch {
        crate::log::trace!(crate::log::system_time(); kind = "is_short_range_fork", msg = format!("same epoch {a_epoch}, staking lock checkpoints {} and {}", a.staking_epoch_data.lock_checkpoint, b.staking_epoch_data.lock_checkpoint));
        a.staking_epoch_data.lock_checkpoint == b.staking_epoch_data.lock_checkpoint
    } else {
        pred_case(a, b) || pred_case(b, a)
    }
}

/// The minimum window density of a chain whose tip is `state`, as if it
/// were extended without blocks to `max_slot`, the later of two tips' slots:
/// the tip's own at that slot, otherwise a virtual block's there.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2864-L2879>
pub fn virtual_min_window_density(state: &MinaConsensusState, max_slot: u32) -> u32 {
    let slot = global_slot(state);
    if slot == max_slot {
        return state.min_window_density.as_u32();
    }
    let densities = state
        .sub_window_densities
        .iter()
        .map(|density| density.as_u32())
        .collect::<Vec<_>>();
    update_min_window_density(
        DensityConstants::FORK_CHOICE,
        slot,
        max_slot,
        &densities,
        state.min_window_density.as_u32(),
        false,
    )
    .0
}

/// The relative minimum window density of `b1` against `b2`: `b1`'s
/// virtual minimum window density at the later of their slots.
pub fn relative_min_window_density(b1: &MinaConsensusState, b2: &MinaConsensusState) -> u32 {
    virtual_min_window_density(b1, global_slot(b1).max(global_slot(b2)))
}

fn global_slot(b: &MinaConsensusState) -> u32 {
    b.curr_global_slot_since_hard_fork.slot_number.as_u32()
}

/// Selection between two tips with the same finalized checkpoint: the
/// longer chain, then the greater Blake2b digest of the truncated VRF
/// output, then the greater state hash; on equality, the existing tip.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2838-L2860>
pub fn short_range_fork_take(
    tip_cs: &MinaConsensusState,
    candidate_cs: &MinaConsensusState,
    tip_hash: &StateHash,
    candidate_hash: &StateHash,
) -> (bool, ConsensusShortRangeForkDecisionReason) {
    use std::cmp::Ordering::*;
    use ConsensusShortRangeForkDecisionReason::*;

    let tip_height = tip_cs.blockchain_length.as_u32();
    let candidate_height = candidate_cs.blockchain_length.as_u32();
    match candidate_height.cmp(&tip_height) {
        Greater => return (true, ChainLength),
        Less => return (false, ChainLength),
        Equal => {}
    }

    let tip_vrf = tip_cs.last_vrf_output.blake2b();
    let candidate_vrf = candidate_cs.last_vrf_output.blake2b();
    match candidate_vrf.cmp(&tip_vrf) {
        Greater => return (true, Vrf),
        Less => return (false, Vrf),
        Equal => {}
    }

    (candidate_hash > tip_hash, StateHash)
}

/// Selection between two tips with different finalized checkpoints: the
/// greater virtual minimum window density at the later of their slots, then
/// as [`short_range_fork_take`].
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2861-L2884>
pub fn long_range_fork_take(
    tip_cs: &MinaConsensusState,
    candidate_cs: &MinaConsensusState,
    tip_hash: &StateHash,
    candidate_hash: &StateHash,
) -> (bool, ConsensusLongRangeForkDecisionReason) {
    use std::cmp::Ordering::*;
    use ConsensusLongRangeForkDecisionReason::*;

    let max_slot = global_slot(tip_cs).max(global_slot(candidate_cs));
    let tip_density = virtual_min_window_density(tip_cs, max_slot);
    let candidate_density = virtual_min_window_density(candidate_cs, max_slot);
    match candidate_density.cmp(&tip_density) {
        Greater => return (true, SubWindowDensity),
        Less => return (false, SubWindowDensity),
        Equal => {}
    }

    let (take, reason) = short_range_fork_take(tip_cs, candidate_cs, tip_hash, candidate_hash);
    let reason = match reason {
        ConsensusShortRangeForkDecisionReason::ChainLength => ChainLength,
        ConsensusShortRangeForkDecisionReason::Vrf => Vrf,
        ConsensusShortRangeForkDecisionReason::StateHash => StateHash,
    };
    (take, reason)
}

/// Whether `candidate` is to replace `tip` as the best tip: OCaml's
/// `select`, `Take` being `true`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2791-L2898>
pub fn consensus_take(
    tip_cs: &MinaConsensusState,
    candidate_cs: &MinaConsensusState,
    tip_hash: &StateHash,
    candidate_hash: &StateHash,
) -> bool {
    if is_short_range_fork(tip_cs, candidate_cs) {
        short_range_fork_take(tip_cs, candidate_cs, tip_hash, candidate_hash).0
    } else {
        long_range_fork_take(tip_cs, candidate_cs, tip_hash, candidate_hash).0
    }
}

/// The constants of the sub-window density ring and its grace period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DensityConstants {
    pub slots_per_sub_window: u32,
    pub sub_windows_per_window: u32,
    /// `grace_period_slots + slots_per_window`, in slots since the hard fork.
    pub grace_period_end: u32,
}

impl DensityConstants {
    /// The constants fork choice uses: mainnet's, as at the top of this
    /// module, which Mesa devnet shares.
    pub const FORK_CHOICE: Self = Self {
        slots_per_sub_window: SLOTS_PER_SUB_WINDOW,
        sub_windows_per_window: SUB_WINDOWS_PER_WINDOW,
        grace_period_end: GRACE_PERIOD_END,
    };
}

impl From<&ConsensusConstants> for DensityConstants {
    fn from(constants: &ConsensusConstants) -> Self {
        Self {
            slots_per_sub_window: constants.slots_per_sub_window,
            sub_windows_per_window: constants.sub_windows_per_window,
            grace_period_end: constants.grace_period_end,
        }
    }
}

/// The minimum window density and the sub-window densities of a block at
/// `next_global_slot` whose parent, at `prev_global_slot`, has
/// `prev_sub_window_densities` and `prev_min_window_density`. With
/// `incr_window` the block counts in its own sub-window, as when a block is
/// made; without, the result is a virtual block's, as fork choice projects a
/// chain to a later slot.
///
/// The previous window is the sub-windows `(prev - n, prev]`, the current
/// one, whose density can lower the minimum, `[next - n, next)`, and the
/// returned one `(next - n, next]`, `n` being `sub_windows_per_window`: the
/// ring keeps the densities of the sub-windows the current window shares with
/// the previous one, endpoints included, and zeroes those in between. Within
/// a sub-window, or before the grace period ends, the minimum is the parent's.
/// `next_global_slot` must not be lower than `prev_global_slot`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1165-L1279>
pub fn update_min_window_density(
    constants: DensityConstants,
    prev_global_slot: u32,
    next_global_slot: u32,
    prev_sub_window_densities: &[u32],
    prev_min_window_density: u32,
    incr_window: bool,
) -> (u32, Vec<u32>) {
    let slots_per_sub_window = constants.slots_per_sub_window.max(1);
    let sub_windows_per_window = constants.sub_windows_per_window.max(1);
    let prev_global_sub_window = prev_global_slot / slots_per_sub_window;
    let next_global_sub_window = next_global_slot / slots_per_sub_window;
    let prev_relative_sub_window = prev_global_sub_window % sub_windows_per_window;
    let next_relative_sub_window = next_global_sub_window % sub_windows_per_window;
    let same_sub_window = prev_global_sub_window == next_global_sub_window;
    let overlapping_window =
        prev_global_sub_window.saturating_add(sub_windows_per_window) >= next_global_sub_window;

    let current_sub_window_densities = prev_sub_window_densities
        .iter()
        .zip(0u32..)
        .map(|(&density, i)| {
            let gt_prev_sub_window = i > prev_relative_sub_window;
            let lt_next_sub_window = i < next_relative_sub_window;
            let within_range = if prev_relative_sub_window < next_relative_sub_window {
                gt_prev_sub_window && lt_next_sub_window
            } else {
                gt_prev_sub_window || lt_next_sub_window
            };
            if same_sub_window || (overlapping_window && !within_range) {
                density
            } else {
                0
            }
        })
        .collect::<Vec<_>>();

    let min_window_density = if same_sub_window || next_global_slot < constants.grace_period_end {
        prev_min_window_density
    } else {
        let current_window_density = current_sub_window_densities
            .iter()
            .fold(0u32, |sum, density| sum.saturating_add(*density));
        current_window_density.min(prev_min_window_density)
    };

    let next_sub_window_densities = current_sub_window_densities
        .into_iter()
        .zip(0u32..)
        .map(|(density, i)| {
            if i == next_relative_sub_window {
                let density = if same_sub_window { density } else { 0 };
                if incr_window {
                    density.saturating_add(1)
                } else {
                    density
                }
            } else {
                density
            }
        })
        .collect();

    (min_window_density, next_sub_window_densities)
}

/// The epoch seed after a block with VRF output hash `vrf_hash` in the seed
/// update range.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L140-L144>
pub fn calc_epoch_seed(
    prev_epoch_seed: &v2::EpochSeed,
    vrf_hash: mina_curves::pasta::Fp,
) -> Result<v2::EpochSeed, String> {
    let old_seed = prev_epoch_seed
        .to_field()
        .map_err(|_| "invalid epoch seed".to_owned())?;
    let new_seed = poseidon::hash::hash_with_kimchi(
        &poseidon::hash::params::MINA_EPOCH_SEED,
        &[old_seed, vrf_hash],
    );
    Ok(v2::MinaBaseEpochSeedStableV1(new_seed.into()).into())
}

/// What a block adds to its parent's consensus state: the arguments of
/// OCaml's `Consensus_state.update` other than the parent's state and hash.
#[derive(Debug, Clone)]
pub struct ConsensusStateUpdate {
    /// The block's slot (OCaml's `consensus_transition`).
    pub curr_global_slot_since_hard_fork: v2::ConsensusGlobalSlotStableV1,
    /// The hash of the block's VRF output (OCaml's `producer_vrf_result`).
    pub vrf_hash: mina_curves::pasta::Fp,
    /// The VRF output truncated.
    pub last_vrf_output: v2::ConsensusVrfOutputTruncatedStableV1,
    /// The parent's snarked ledger hash.
    pub snarked_ledger_hash: v2::LedgerHash,
    pub genesis_ledger_hash: v2::LedgerHash,
    /// The parent's total currency plus the block's supply increase.
    pub total_currency: v2::CurrencyAmountStableV1,
    pub block_stake_winner: v2::NonZeroCurvePoint,
    pub block_creator: v2::NonZeroCurvePoint,
    pub coinbase_receiver: v2::NonZeroCurvePoint,
    pub supercharge_coinbase: bool,
}

/// The consensus state of a block made on top of `prev`, whose state hash is
/// `prev_state_hash`: OCaml's `Consensus_state.update`, which its checked
/// counterpart, `update_var`, enforces in the block proof.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1855-L1955>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1062-L1109>
pub fn next_consensus_state(
    constants: &ConsensusConstants,
    prev: &MinaConsensusState,
    prev_state_hash: &StateHash,
    update: ConsensusStateUpdate,
) -> Result<MinaConsensusState, String> {
    let ConsensusStateUpdate {
        curr_global_slot_since_hard_fork,
        vrf_hash,
        last_vrf_output,
        snarked_ledger_hash,
        genesis_ledger_hash,
        total_currency,
        block_stake_winner,
        block_creator,
        coinbase_receiver,
        supercharge_coinbase,
    } = update;

    let prev_global_slot = &prev.curr_global_slot_since_hard_fork;
    let prev_slot = prev_global_slot.slot_number.as_u32();
    let next_slot = curr_global_slot_since_hard_fork.slot_number.as_u32();
    // Only the genesis transition, to slot 0, may keep the slot.
    let Some(slot_diff) = next_slot
        .checked_sub(prev_slot)
        .filter(|diff| *diff > 0 || next_slot == 0)
    else {
        return Err(format!(
            "slot did not increase: from {prev_slot} to {next_slot}"
        ));
    };
    let (prev_epoch, _) = curr_epoch_and_slot(prev);
    let next_slots_per_epoch = curr_global_slot_since_hard_fork
        .slots_per_epoch
        .as_u32()
        .max(1);
    let next_epoch = next_slot / next_slots_per_epoch;
    let next_slot_in_epoch = next_slot % next_slots_per_epoch;

    // `Epoch_data.update_pair`.
    let next_staking_ledger = if snarked_ledger_hash == genesis_ledger_hash {
        prev.next_epoch_data.ledger.clone()
    } else {
        v2::MinaBaseEpochLedgerValueStableV1 {
            hash: snarked_ledger_hash,
            total_currency: total_currency.clone(),
        }
    };
    let (staking_epoch_data, next_epoch_data, epoch_count) = if next_epoch > prev_epoch {
        let next = &prev.next_epoch_data;
        let staking_epoch_data =
            v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 {
                seed: next.seed.clone(),
                ledger: next.ledger.clone(),
                start_checkpoint: next.start_checkpoint.clone(),
                lock_checkpoint: next.lock_checkpoint.clone(),
                epoch_length: next.epoch_length,
            };
        let next_epoch_data =
            v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
                seed: next.seed.clone(),
                ledger: next_staking_ledger,
                start_checkpoint: prev_state_hash.clone(),
                // OCaml: (* TODO: We need to make sure issue #2328 is properly addressed. *)
                lock_checkpoint: StateHash::zero(),
                epoch_length: 1u32.into(),
            };
        let epoch_count = prev
            .epoch_count
            .as_u32()
            .checked_add(1)
            .ok_or("epoch count overflow")?;
        (staking_epoch_data, next_epoch_data, epoch_count.into())
    } else if next_epoch == prev_epoch {
        let mut next_epoch_data = prev.next_epoch_data.clone();
        next_epoch_data.epoch_length = next_epoch_data
            .epoch_length
            .as_u32()
            .checked_add(1)
            .ok_or("epoch length overflow")?
            .into();
        (
            prev.staking_epoch_data.clone(),
            next_epoch_data,
            prev.epoch_count,
        )
    } else {
        return Err(format!(
            "epoch decreased: from {prev_epoch} to {next_epoch}"
        ));
    };
    // `Slot.in_seed_update_range`.
    let next_epoch_data = if next_slot_in_epoch < constants.slots_per_epoch / 3 * 2 {
        v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
            seed: calc_epoch_seed(&next_epoch_data.seed, vrf_hash)?,
            lock_checkpoint: prev_state_hash.clone(),
            ..next_epoch_data
        }
    } else {
        next_epoch_data
    };

    let prev_sub_window_densities = prev
        .sub_window_densities
        .iter()
        .map(|density| density.as_u32())
        .collect::<Vec<_>>();
    let (min_window_density, sub_window_densities) = update_min_window_density(
        constants.into(),
        prev_slot,
        next_slot,
        &prev_sub_window_densities,
        prev.min_window_density.as_u32(),
        true,
    );

    let checkpoint_window_size_in_slots = constants.checkpoint_window_size_in_slots.max(1);
    let has_ancestor_in_same_checkpoint_window =
        prev_slot / checkpoint_window_size_in_slots == next_slot / checkpoint_window_size_in_slots;
    let global_slot_since_genesis = prev
        .global_slot_since_genesis
        .as_u32()
        .checked_add(slot_diff)
        .ok_or("global slot since genesis overflow")?;

    Ok(MinaConsensusState {
        blockchain_length: prev
            .blockchain_length
            .as_u32()
            .checked_add(1)
            .ok_or("blockchain length overflow")?
            .into(),
        epoch_count,
        min_window_density: min_window_density.into(),
        sub_window_densities: sub_window_densities.into_iter().map(Into::into).collect(),
        last_vrf_output,
        total_currency,
        curr_global_slot_since_hard_fork,
        global_slot_since_genesis: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
            global_slot_since_genesis.into(),
        ),
        staking_epoch_data,
        next_epoch_data,
        has_ancestor_in_same_checkpoint_window,
        block_stake_winner,
        block_creator,
        coinbase_receiver,
        supercharge_coinbase,
    })
}

pub fn in_seed_update_range(
    slot: u32,
    constants: &v2::MinaBaseProtocolConstantsCheckedValueStableV1,
) -> bool {
    let third_epoch = constants.slots_per_epoch.as_u32() / 3;
    assert_eq!(constants.slots_per_epoch.as_u32(), third_epoch * 3);
    slot < third_epoch * 2
}

pub fn in_same_checkpoint_window(
    slot1: &v2::ConsensusGlobalSlotStableV1,
    slot2: &v2::ConsensusGlobalSlotStableV1,
) -> bool {
    checkpoint_window(slot1) == checkpoint_window(slot2)
}

pub fn checkpoint_window(slot: &v2::ConsensusGlobalSlotStableV1) -> u32 {
    slot.slot_number.as_u32() / checkpoint_window_size_in_slots()
}

pub fn global_sub_window(
    slot: &v2::ConsensusGlobalSlotStableV1,
    constants: &v2::MinaBaseProtocolConstantsCheckedValueStableV1,
) -> u32 {
    slot.slot_number.as_u32() / constants.slots_per_sub_window.as_u32()
}

pub fn relative_sub_window(global_sub_window: u32) -> u32 {
    global_sub_window % constraint_constants().sub_windows_per_window as u32
}

// TODO: Move ledger/src/scan_state/currency.rs types to core and replace
// primmitive types here with thoise numeric types.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsensusConstants {
    pub k: u32,
    pub delta: u32,
    pub block_window_duration_ms: u64,
    pub slots_per_sub_window: u32,
    pub slots_per_window: u32,
    pub sub_windows_per_window: u32,
    pub slots_per_epoch: u32,
    pub grace_period_slots: u32,
    pub grace_period_end: u32,
    pub slot_duration_ms: u64,
    pub epoch_duration: u64,
    pub checkpoint_window_slots_per_year: u32,
    pub checkpoint_window_size_in_slots: u32,
    pub delta_duration: u64,
    pub genesis_state_timestamp: BlockTimeTimeStableV1,
}

impl ConsensusConstants {
    // We mimick the code layout of the OCaml node's here. `create_primed` could easily
    // be inlined in `create`, but OCaml code keeps them separate ans so do we for now.
    fn create_primed(
        constraint_constants: &crate::constants::ConstraintConstants,
        protocol_constants: &v2::MinaBaseProtocolConstantsCheckedValueStableV1,
    ) -> Self {
        let delta = protocol_constants.delta.as_u32();
        let slots_per_epoch = protocol_constants.slots_per_epoch.as_u32();
        let slots_per_window = protocol_constants.slots_per_sub_window.as_u32()
            * constraint_constants.sub_windows_per_window as u32;
        let grace_period_end = protocol_constants.grace_period_slots.as_u32() + slots_per_window;
        let epoch_duration =
            (slots_per_epoch as u64) * constraint_constants.block_window_duration_ms;
        let delta_duration = constraint_constants.block_window_duration_ms * (delta + 1) as u64;
        Self {
            k: protocol_constants.k.as_u32(),
            delta,
            block_window_duration_ms: constraint_constants.block_window_duration_ms,
            slots_per_sub_window: protocol_constants.slots_per_sub_window.as_u32(),
            slots_per_window,
            sub_windows_per_window: constraint_constants.sub_windows_per_window as u32,
            slots_per_epoch,
            grace_period_slots: protocol_constants.grace_period_slots.as_u32(),
            grace_period_end,
            slot_duration_ms: constraint_constants.block_window_duration_ms,
            epoch_duration,
            checkpoint_window_slots_per_year: 0,
            checkpoint_window_size_in_slots: 0,
            delta_duration,
            genesis_state_timestamp: protocol_constants.genesis_state_timestamp,
        }
    }

    pub fn assert_invariants(&self) {
        let grace_period_effective_end = self.grace_period_end - self.slots_per_window;
        assert!(grace_period_effective_end < (self.slots_per_epoch / 3));
        // Because of how these values are computed (see below), this
        // fails if and only if block_window_duration is a multiple of
        // 27 or 512, or any of these multiplied by a power of 3 or 2
        // respectively.
        // 365 * 24 * 60 * 60 * 1000 = 2^10 * 3^3 * 5^6 * 73
        // Therefore, if divided by 2^9 or 3^3, the whole value will not be
        // divisible by 12 (2^2 * 3) anymore.
        assert_eq!(
            self.checkpoint_window_slots_per_year as u64,
            self.checkpoint_window_size_in_slots as u64 * CHECKPOINTS_PER_YEAR
        )
    }

    pub fn create(
        constraint_constants: &crate::constants::ConstraintConstants,
        protocol_constants: &v2::MinaBaseProtocolConstantsCheckedValueStableV1,
    ) -> Self {
        let mut constants = Self::create_primed(constraint_constants, protocol_constants);
        const MILLISECS_PER_YEAR: u64 = 365 * 24 * 60 * 60 * 1000;
        let slots_per_year = MILLISECS_PER_YEAR / constants.block_window_duration_ms;
        constants.checkpoint_window_slots_per_year = slots_per_year as u32;
        constants.checkpoint_window_size_in_slots = (slots_per_year / CHECKPOINTS_PER_YEAR) as u32;
        constants.assert_invariants();
        constants
    }

    pub fn human_readable_genesis_timestamp(&self) -> Result<String, String> {
        let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6][offset_hour sign:mandatory]:[offset_minute]");
        OffsetDateTime::from_unix_timestamp((self.genesis_state_timestamp.as_u64() / 1000) as i64)
            .map_err(|e| e.to_string())
            .and_then(|dt| dt.format(&format).map_err(|e| e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    //! The fork fixtures are protocol states from a 2023 Berkeley test network
    //! (30-slot epochs). Each file name gives the fork rule and the decision;
    //! a transcription of OCaml 4.0.2's `select` and `is_short_range`, with
    //! the mainnet constants this module hard-codes, gives the same for every
    //! pair. Only their consensus states
    //! are read: the rest predates Mesa's protocol constants.

    use super::{
        consensus_take, is_short_range_fork, long_range_fork_take, short_range_fork_take,
        MinaConsensusState,
    };
    use mina_p2p_messages::v2::StateHash;

    fn consensus_state(protocol_state_json: &str) -> MinaConsensusState {
        let mut state: serde_json::Value = serde_json::from_str(protocol_state_json).unwrap();
        serde_json::from_value(state["body"]["consensus_state"].take()).unwrap()
    }

    macro_rules! fork_file {
        ($prefix:expr, $tip:expr, $cnd:expr, $suffix:expr) => {
            concat!(
                "../../../tests/files/forks/",
                $prefix,
                "-",
                $tip,
                "-",
                $cnd,
                "-",
                $suffix,
                ".json"
            )
        };
    }
    macro_rules! fork_test {
        ($prefix:expr, $tip:expr, $cnd:expr, $func:ident, $decision:expr, $short:expr) => {
            let tip_str = include_str!(fork_file!($prefix, $tip, $cnd, "tip"));
            let cnd_str = include_str!(fork_file!($prefix, $tip, $cnd, "cnd"));
            let tip_hash = $tip.parse::<StateHash>().unwrap();
            let cnd_hash = $cnd.parse::<StateHash>().unwrap();
            let (tip_cs, cnd_cs) = (&consensus_state(tip_str), &consensus_state(cnd_str));

            let (take, _) = $func(tip_cs, cnd_cs, &tip_hash, &cnd_hash);
            assert_eq!(take, $decision);
            assert_eq!(is_short_range_fork(tip_cs, cnd_cs), $short);
            assert_eq!(is_short_range_fork(cnd_cs, tip_cs), $short);
            assert_eq!(
                consensus_take(tip_cs, cnd_cs, &tip_hash, &cnd_hash),
                $decision
            );
        };

        (long take $prefix:expr, $tip:expr, $cnd:expr) => {
            fork_test!(
                concat!("long-take-", $prefix),
                $tip,
                $cnd,
                long_range_fork_take,
                true,
                false
            );
        };

        (long keep $prefix:expr, $tip:expr, $cnd:expr) => {
            fork_test!(
                concat!("long-keep-", $prefix),
                $tip,
                $cnd,
                long_range_fork_take,
                false,
                false
            );
        };

        (short take $prefix:expr, $tip:expr, $cnd:expr) => {
            fork_test!(
                concat!("short-take-", $prefix),
                $tip,
                $cnd,
                short_range_fork_take,
                true,
                true
            );
        };

        (short keep $prefix:expr, $tip:expr, $cnd:expr) => {
            fork_test!(
                concat!("short-keep-", $prefix),
                $tip,
                $cnd,
                short_range_fork_take,
                false,
                true
            );
        };
    }

    #[test]
    fn long_range_fork() {
        fork_test!(
            long take
                "density-92-97",
            "3NLESd9gzU52bDWSXL5uUAYbCojHXSVdeBX4sCMF3V8Ns9D1Sriy",
            "3NLQfKJ4kBagLgmiwyiVw9zbi53tiNy8TNu2ua1jmCyEecgbBJoN"
        );
        fork_test!(
            long keep
                "density-161-166",
            "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG",
            "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC"
        );
    }

    #[test]
    fn short_range_fork() {
        fork_test!(
            short take
                "length-60-61",
            "3NLQEb5mXqXCL34rueHrMkUVyWSQ7aYjvi6K98ZdpEnTozef69uR",
            "3NKuw8mvieV9RLpdRmHb4kxg7NWR83TfwzNkVmJCeHUmVWFdUQCp"
        );
        fork_test!(
            short take
                "vrf-99-99",
                "3NL4kAA33FRs9K66GvVNupNT94L4shALtYLHJRfmxhdZV8iPg2pi",
                "3NKC9F6mgtvRiHgYxiPBt1P5QDYaPVpD3YWyJhjmJZkNnT7RYitm"
        );
        fork_test!(
            short keep
                "vrf-117-117",
                "3NLWvDBFYJ2NXZ1EKMZXHB52zcbVtosHPArn4cGj8pDKkYsTHNnC",
                "3NKLEnUBTAhC95XEdJpLvJPqAUuvkC176tFKyLDcXUcofXXgQUvY"
        );
    }

    /// The fork choice constants hard-coded at the top of this file must be
    /// mainnet's, which `tests/ocaml_mainnet_constants.rs` checks against
    /// OCaml (P11d).
    #[test]
    fn fork_choice_constants_are_mainnet_ones() {
        use crate::{constants::PROTOCOL_CONSTANTS, network::mainnet::CONSTRAINT_CONSTANTS};

        assert_eq!(
            super::SUB_WINDOWS_PER_WINDOW as u64,
            CONSTRAINT_CONSTANTS.sub_windows_per_window
        );
        assert_eq!(
            super::SLOTS_PER_SUB_WINDOW,
            PROTOCOL_CONSTANTS.slots_per_sub_window.as_u32()
        );
        assert_eq!(
            super::GRACE_PERIOD_SLOTS,
            PROTOCOL_CONSTANTS.grace_period_slots.as_u32()
        );
    }

    /// OCaml's `update_min_window_density` (proof_of_stake.ml:1165-1279)
    /// from slot 7151, global sub-window 1021, ring index 9, past the grace
    /// period, with buckets `1..=11` (sum 66) and minimum 70: the ring keeps
    /// the sub-windows the new window shares with the old, both ends
    /// included, and zeroes those in between.
    #[test]
    fn min_window_density_ring_projection() {
        use super::{update_min_window_density, DensityConstants};
        let c = DensityConstants::FORK_CHOICE;
        let buckets: Vec<u32> = (1..=11).collect();
        let at = |next, incr| update_min_window_density(c, 7151, next, &buckets, 70, incr);

        // The same sub-window: the minimum is kept, the block counts there.
        assert_eq!(at(7152, false), (70, buckets.clone()));
        let mut counted = buckets.clone();
        counted[9] += 1;
        assert_eq!(at(7152, true), (70, counted));

        // The next sub-window, 1022 at index 10: the current window,
        // 1011 to 1021, is every bucket; 1011's, index 10, is then reset.
        let mut next = buckets.clone();
        next[10] = 0;
        assert_eq!(at(7157, false), (66, next.clone()));
        next[10] = 1;
        assert_eq!(at(7157, true), (66, next));

        // Two sub-windows on, 1023 at index 0: 1022, index 10, is empty.
        let mut skipped = buckets.clone();
        skipped[10] = 0;
        let sum = skipped.iter().sum::<u32>();
        skipped[0] = 0;
        assert_eq!(at(7161, false), (sum, skipped));

        // Exactly a window on, 1032 at index 9 again: only 1021 is shared,
        // and then reset for 1032.
        assert_eq!(at(1032 * 7, false), (10, vec![0; 11]));

        // Further: nothing is shared.
        assert_eq!(at(1033 * 7, false), (0, vec![0; 11]));

        // Within the grace period the minimum stays.
        let early = update_min_window_density(c, 70, 2236, &buckets, 70, false);
        assert_eq!(early, (70, vec![0; 11]));
    }

    /// At the later of two slots a tip's virtual minimum window density is
    /// its own minimum (proof_of_stake.ml:2868-2870). The P4.9 review's
    /// example: equal slots 7151, different staking lock checkpoints, minima
    /// 20 and 21. OCaml takes the candidate; Rust's projection used to clear
    /// a bucket of each and keep the tip, 20 against 14.
    #[test]
    fn long_range_equal_slots_compare_stored_minima() {
        let mut tip = consensus_state(include_str!(fork_file!(
            "long-keep-density-161-166",
            "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG",
            "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC",
            "tip"
        )));
        let mut candidate = consensus_state(include_str!(fork_file!(
            "long-keep-density-161-166",
            "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG",
            "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC",
            "cnd"
        )));
        for state in [&mut tip, &mut candidate] {
            state.curr_global_slot_since_hard_fork.slot_number =
                mina_p2p_messages::v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
                    7151u32.into(),
                );
            state.curr_global_slot_since_hard_fork.slots_per_epoch = 7140u32.into();
        }
        tip.min_window_density = 20u32.into();
        tip.sub_window_densities = [2u32; 11].into_iter().map(Into::into).collect();
        candidate.min_window_density = 21u32.into();
        candidate.sub_window_densities = [1u32, 2, 1, 2, 1, 2, 1, 2, 1, 1, 7]
            .into_iter()
            .map(Into::into)
            .collect();
        assert!(!is_short_range_fork(&tip, &candidate));
        assert_eq!(super::relative_min_window_density(&tip, &candidate), 20);
        assert_eq!(super::relative_min_window_density(&candidate, &tip), 21);
        let tip_hash: StateHash = "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG"
            .parse()
            .unwrap();
        let candidate_hash: StateHash = "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC"
            .parse()
            .unwrap();
        assert!(consensus_take(&tip, &candidate, &tip_hash, &candidate_hash));
        assert!(!consensus_take(
            &candidate,
            &tip,
            &candidate_hash,
            &tip_hash
        ));
    }
}
