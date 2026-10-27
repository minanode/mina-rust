//! The next hard fork's genesis inputs, from this node's own state: the fork
//! block, its slots and epoch data, and the ledgers the next chain starts
//! from, as OCaml 4.0.2's `mina advanced generate-hardfork-config` and GraphQL
//! `fork_config` derive them.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2704-L2987>

use std::{collections::BTreeMap, sync::Arc};

use mina_core::{block::AppliedBlock, constants::StopSlots};
use mina_p2p_messages::v2::{
    EpochSeed, LedgerHash, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2,
    StateHash,
};
use serde::{Deserialize, Serialize};

/// Which block the next chain continues from.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2742-L2754>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HardForkBlockSpec {
    /// The newest best-chain block before `slot_tx_end`, or the best tip
    /// without one. What `generate-hardfork-config` and the auto mode use.
    StopSlot,
    /// A best-chain block by state hash (GraphQL `fork_config(stateHash)`).
    StateHash(StateHash),
    /// A best-chain block by height (GraphQL `fork_config(height)`).
    Height(u32),
}

/// One best-chain block, as the export reports it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkBlock {
    pub state_hash: StateHash,
    pub blockchain_length: u32,
    pub curr_global_slot_since_hard_fork: u32,
    pub global_slot_since_genesis: u32,
    pub epoch: u32,
    /// The staged ledger's Merkle root (`non_snark.ledger_hash`).
    pub staged_ledger_hash: LedgerHash,
    pub snarked_ledger_hash: LedgerHash,
}

impl HardForkBlock {
    fn of_block(block: &AppliedBlock) -> Self {
        Self {
            state_hash: block.hash().clone(),
            blockchain_length: block.height(),
            curr_global_slot_since_hard_fork: block.global_slot(),
            global_slot_since_genesis: block.global_slot_since_genesis(),
            epoch: block.consensus_state().epoch_count.as_u32(),
            staged_ledger_hash: block.merkle_root_hash().clone(),
            snarked_ledger_hash: block.snarked_ledger_hash().clone(),
        }
    }
}

/// `proof.fork` of the next chain's runtime config.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L3-L11>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HardForkPoint {
    pub state_hash: StateHash,
    pub blockchain_length: u32,
    pub global_slot_since_genesis: u32,
}

/// A ledger the next chain starts from, with its seed for epoch ledgers.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkEpochInput {
    pub ledger_hash: LedgerHash,
    pub seed: EpochSeed,
}

/// Everything the next chain's runtime config needs except the accounts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkInputs {
    pub spec: HardForkBlockSpec,
    pub stop_slots: StopSlots,
    pub best_tip: HardForkBlock,
    /// Whether the best tip is at or past `slot_tx_end`, so no other block
    /// before it can still arrive. OCaml's auto mode waits for this.
    pub stop_reached: bool,
    pub fork_block: HardForkBlock,
    /// The next chain's genesis slot, in this chain's slots since its hard
    /// fork: scheduled, or the fork block's own slot.
    pub genesis_slot_since_hard_fork: u32,
    pub fork: HardForkPoint,
    /// `genesis.genesis_state_timestamp`, as OCaml prints it.
    pub genesis_state_timestamp: String,
    pub genesis_state_timestamp_ms: u64,
    pub staking_epoch: HardForkEpochInput,
    pub next_epoch: HardForkEpochInput,
}

/// The newest block of `best_chain` (root first) whose slot since the hard
/// fork is below `slot_tx_end`; the best tip if there is no `slot_tx_end`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2704-L2737>
pub fn block_before_stop_slot(
    best_chain: &[AppliedBlock],
    stops: StopSlots,
) -> Result<&AppliedBlock, String> {
    let best_tip = best_chain.last().ok_or("the node has no best tip yet")?;
    let Some(slot_tx_end) = stops.slot_tx_end else {
        return Ok(best_tip);
    };
    best_chain
        .iter()
        .rev()
        .find(|block| block.global_slot() < slot_tx_end)
        .ok_or_else(|| {
            format!(
                "no block of the transition frontier is before slot_tx_end {slot_tx_end}; \
                 its root is at slot {}",
                best_chain[0].global_slot()
            )
        })
}

/// The next chain's genesis slot since genesis: the fork block's slot since
/// genesis, moved forward as far as its slot since the hard fork moves to
/// the scheduled genesis slot.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2908-L2944>
pub fn scheduled_global_slot_since_genesis(
    block_slot_since_hard_fork: u32,
    block_slot_since_genesis: u32,
    genesis_slot_since_hard_fork: u32,
) -> Result<u32, String> {
    let span = genesis_slot_since_hard_fork
        .checked_sub(block_slot_since_hard_fork)
        .ok_or_else(|| {
            format!(
                "the hard fork genesis slot {genesis_slot_since_hard_fork} is before the fork \
                 block's slot {block_slot_since_hard_fork}"
            )
        })?;
    block_slot_since_genesis
        .checked_add(span)
        .ok_or_else(|| "the hard fork genesis slot overflows".to_owned())
}

/// The start of `slot` (since this hard fork) on this chain's clock, in
/// milliseconds since the epoch.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2882-L2887>
pub fn slot_start_ms(genesis_timestamp_ms: u64, slot_duration_ms: u64, slot: u32) -> u64 {
    genesis_timestamp_ms.saturating_add(slot_duration_ms.saturating_mul(slot as u64))
}

/// `Time.to_string_iso8601_basic ~zone:Time.Zone.utc`:
/// `2026-08-19T18:00:00.000000Z`.
pub fn iso8601_basic_utc(ms: u64) -> Result<String, String> {
    use time::OffsetDateTime;
    let time = OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .map_err(|e| format!("timestamp {ms} ms out of range: {e}"))?;
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        time.microsecond()
    ))
}

/// The fork inputs at `spec`, from the best chain (root first), the runtime
/// stop slots and the slot duration.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2946-L2987>
pub fn hard_fork_inputs(
    best_chain: &[AppliedBlock],
    stops: StopSlots,
    slot_duration_ms: u64,
    spec: &HardForkBlockSpec,
) -> Result<HardForkInputs, String> {
    let best_tip = best_chain.last().ok_or("the node has no best tip yet")?;
    let block = match spec {
        HardForkBlockSpec::StopSlot => block_before_stop_slot(best_chain, stops)?,
        HardForkBlockSpec::StateHash(hash) => best_chain
            .iter()
            .find(|b| b.hash() == hash)
            .ok_or_else(|| format!("block {hash} is not in the best chain of the frontier"))?,
        HardForkBlockSpec::Height(height) => best_chain
            .iter()
            .find(|b| b.height() == *height)
            .ok_or_else(|| format!("no block at height {height} in the frontier's best chain"))?,
    };
    // Only the stop-slot spec uses the schedule, as in OCaml.
    let genesis_slot_since_hard_fork = match spec {
        HardForkBlockSpec::StopSlot => stops.scheduled_hard_fork_genesis_slot(),
        _ => None,
    }
    .unwrap_or_else(|| block.global_slot());
    let global_slot_since_genesis = scheduled_global_slot_since_genesis(
        block.global_slot(),
        block.global_slot_since_genesis(),
        genesis_slot_since_hard_fork,
    )?;
    let genesis_state_timestamp_ms = slot_start_ms(
        block.constants().genesis_state_timestamp.0.as_u64(),
        slot_duration_ms,
        genesis_slot_since_hard_fork,
    );
    let consensus = block.consensus_state();
    Ok(HardForkInputs {
        spec: spec.clone(),
        stop_slots: stops,
        best_tip: HardForkBlock::of_block(best_tip),
        stop_reached: stops
            .slot_tx_end
            .is_none_or(|end| best_tip.global_slot() >= end),
        fork_block: HardForkBlock::of_block(block),
        genesis_slot_since_hard_fork,
        fork: HardForkPoint {
            state_hash: block.hash().clone(),
            blockchain_length: block.height(),
            global_slot_since_genesis,
        },
        genesis_state_timestamp: iso8601_basic_utc(genesis_state_timestamp_ms)?,
        genesis_state_timestamp_ms,
        staking_epoch: HardForkEpochInput {
            ledger_hash: consensus.staking_epoch_data.ledger.hash.clone(),
            seed: consensus.staking_epoch_data.seed.clone(),
        },
        next_epoch: HardForkEpochInput {
            ledger_hash: consensus.next_epoch_data.ledger.hash.clone(),
            seed: consensus.next_epoch_data.seed.clone(),
        },
    })
}

/// What the ledger service needs to export the three ledgers.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkExportRequest {
    pub inputs: HardForkInputs,
    /// This chain's id, for the export's provenance.
    pub chain_id: Option<String>,
    /// The best chain, root first, for rebuilding a snarked epoch ledger the
    /// service doesn't hold (see [`super::LedgerExportRequest::chain`]).
    pub chain: Vec<(LedgerHash, MinaBaseStagedLedgerHashStableV1)>,
    pub protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
}

/// One exported ledger: its accounts and both root hashes.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HardForkLedger {
    /// The Merkle root, recomputed from the exported ledger.
    pub hash: LedgerHash,
    /// The root OCaml 4.0.2 writes in its migrated `daemon.json`: the same
    /// accounts, with empty leaves hashed as `Account.Hardfork.empty`.
    pub migrated_hash: LedgerHash,
    pub num_accounts: u64,
    /// `Runtime_config.Accounts.t` JSON, as [`crate::daemon_json::write_accounts`]
    /// writes it.
    pub accounts_json: String,
}

/// The export: the inputs and the genesis, staking and next epoch ledgers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcHardForkExport {
    pub inputs: HardForkInputs,
    pub chain_id: Option<String>,
    pub ledger: HardForkLedger,
    pub staking: HardForkLedger,
    pub next: HardForkLedger,
}

pub type RpcHardForkExportResponse = Result<Arc<RpcHardForkExport>, String>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Mesa devnet's runtime config, written by OCaml from the Berkeley
    /// chain: the fork block `3NLYmfj4…` is at slot 859399 since genesis,
    /// 413539 since Berkeley's fork (whose genesis slot is 445860). The
    /// Mesa genesis slot is 859560 at `2026-08-19T18:00:00Z`; Berkeley's
    /// genesis is `2024-04-09T21:00:00Z`, with 180 s slots. See R13.
    #[test]
    fn mesa_devnet_genesis_slot_and_timestamp() {
        let berkeley_fork_slot = 445_860;
        let block_since_genesis = 859_399;
        let block_since_hard_fork = block_since_genesis - berkeley_fork_slot;
        let scheduled = 859_560 - berkeley_fork_slot;
        assert_eq!(
            scheduled_global_slot_since_genesis(
                block_since_hard_fork,
                block_since_genesis,
                scheduled
            ),
            Ok(859_560)
        );
        // 2024-04-09T21:00:00Z
        let berkeley_genesis_ms = 1_712_696_400_000;
        let ms = slot_start_ms(berkeley_genesis_ms, 180_000, scheduled);
        // 2026-08-19T18:00:00Z, the Mesa devnet `genesis_state_timestamp`.
        assert_eq!(ms, 1_787_162_400_000);
        assert_eq!(
            iso8601_basic_utc(ms).unwrap(),
            "2026-08-19T18:00:00.000000Z"
        );
    }

    #[test]
    fn genesis_before_fork_block_is_an_error() {
        assert!(scheduled_global_slot_since_genesis(10, 100, 9).is_err());
        assert_eq!(scheduled_global_slot_since_genesis(10, 100, 10), Ok(100));
    }

    #[test]
    fn iso8601_keeps_milliseconds() {
        assert_eq!(
            iso8601_basic_utc(1_787_162_400_123).unwrap(),
            "2026-08-19T18:00:00.123000Z"
        );
    }
}
