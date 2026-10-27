pub mod vrf_evaluator;

mod block_producer_config;
use std::sync::Arc;

pub use block_producer_config::*;

mod block_producer_state;
pub use block_producer_state::*;

mod block_producer_event;
pub use block_producer_event::*;

mod block_producer_actions;
pub use block_producer_actions::*;

mod block_producer_reducer;

mod block_producer_build;
pub use block_producer_build::*;

#[cfg(test)]
mod block_producer_tests;

use ledger::AccountIndex;
use mina_core::{block::ArcBlockWithHash, constants::constraint_constants};
use mina_p2p_messages::{list::List, v2};
use mina_vrf::output::VrfOutput;
use serde::{Deserialize, Serialize};

use self::vrf_evaluator::VrfWonSlotWithHash;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct BlockProducerWonSlot {
    pub slot_time: redux::Timestamp,
    pub delegator: (v2::NonZeroCurvePoint, AccountIndex),
    pub global_slot: v2::ConsensusGlobalSlotStableV1,
    pub vrf_output: Box<VrfOutput>,
    pub value_with_threshold: Option<(f64, f64)>,
    // Staking ledger which was used during vrf evaluation.
    pub staking_ledger_hash: v2::LedgerHash,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockWithoutProof {
    pub protocol_state: v2::MinaStateProtocolStateValueStableV2,
    pub delta_block_chain_proof: (v2::StateHash, List<v2::StateBodyHash>),
    pub current_protocol_version: v2::ProtocolVersionStableV2,
    pub proposed_protocol_version_opt: Option<v2::ProtocolVersionStableV2>,
    pub body: v2::StagedLedgerDiffBodyStableV1,
}

impl BlockProducerWonSlot {
    pub fn from_vrf_won_slot(
        won_slot_with_hash: &VrfWonSlotWithHash,
        genesis_timestamp: redux::Timestamp,
    ) -> Self {
        let VrfWonSlotWithHash {
            won_slot,
            staking_ledger_hash,
        } = won_slot_with_hash;

        let slot_time = Self::calculate_slot_time(genesis_timestamp, won_slot.global_slot);

        let delegator = (
            won_slot.winner_account.clone().into(),
            won_slot.account_index,
        );
        let global_slot = v2::ConsensusGlobalSlotStableV1 {
            slot_number: v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
                won_slot.global_slot.into(),
            ),
            slots_per_epoch: 7140.into(), // TODO
        };

        Self {
            slot_time,
            delegator,
            global_slot,
            vrf_output: won_slot.vrf_output.clone(),
            value_with_threshold: won_slot.value_with_threshold,
            staking_ledger_hash: staking_ledger_hash.clone(),
        }
    }

    fn calculate_slot_time(genesis_timestamp: redux::Timestamp, slot: u32) -> redux::Timestamp {
        let per_block_ns = slot_duration_ns();
        genesis_timestamp
            .checked_add((slot as u64).checked_mul(per_block_ns).expect("overflow"))
            .expect("overflow")
    }

    pub fn global_slot(&self) -> u32 {
        self.global_slot.slot_number.as_u32()
    }

    pub fn epoch(&self) -> u32 {
        self.global_slot()
            .checked_div(self.global_slot.slots_per_epoch.as_u32())
            .expect("division by 0")
    }

    pub fn global_slot_since_genesis(
        &self,
        slot_diff: u32,
    ) -> v2::MinaNumbersGlobalSlotSinceGenesisMStableV1 {
        let slot = self.global_slot().checked_add(slot_diff).expect("overflow");
        v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(slot.into())
    }

    pub fn timestamp(&self) -> v2::BlockTimeTimeStableV1 {
        let ms = u64::from(self.slot_time) / 1_000_000;
        v2::BlockTimeTimeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
            ms.into(),
        ))
    }

    pub fn next_slot_time(&self) -> redux::Timestamp {
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/constants.ml#L229-L256>
        self.slot_time
            .checked_add(slot_duration_ns())
            .expect("overflow")
    }
}

impl PartialOrd for BlockProducerWonSlot {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.global_slot().cmp(&other.global_slot()).then_with(|| {
            v2::ConsensusVrfOutputTruncatedStableV1::from(&*self.vrf_output)
                .blake2b()
                .cmp(&v2::ConsensusVrfOutputTruncatedStableV1::from(&*other.vrf_output).blake2b())
        }))
    }
}

impl PartialEq<ArcBlockWithHash> for BlockProducerWonSlot {
    fn eq(&self, other: &ArcBlockWithHash) -> bool {
        self.partial_cmp(other).is_some_and(|ord| ord.is_eq())
    }
}

impl PartialOrd<ArcBlockWithHash> for BlockProducerWonSlot {
    fn partial_cmp(&self, other: &ArcBlockWithHash) -> Option<std::cmp::Ordering> {
        // TODO(binier): this assumes short range fork
        Some(self.global_slot().cmp(&other.global_slot()).then_with(|| {
            v2::ConsensusVrfOutputTruncatedStableV1::from(&*self.vrf_output)
                .blake2b()
                .cmp(
                    &other
                        .header()
                        .protocol_state
                        .body
                        .consensus_state
                        .last_vrf_output
                        .blake2b(),
                )
        }))
    }
}

/// How long a slot lasts, in nanoseconds: the block window duration, 90 s on
/// Mesa (180 s before).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L155-L166>
pub fn slot_duration_ns() -> u64 {
    constraint_constants()
        .block_window_duration_ms
        .saturating_mul(1_000_000)
}

pub fn to_epoch_and_slot(global_slot: &v2::ConsensusGlobalSlotStableV1) -> (u32, u32) {
    let epoch = global_slot
        .slot_number
        .as_u32()
        .checked_div(global_slot.slots_per_epoch.as_u32())
        .expect("division by 0");
    let slot = global_slot
        .slot_number
        .as_u32()
        .checked_rem(global_slot.slots_per_epoch.as_u32())
        .expect("division by 0");
    (epoch, slot)
}

pub fn next_epoch_first_slot(global_slot: &v2::ConsensusGlobalSlotStableV1) -> u32 {
    let (epoch, _) = to_epoch_and_slot(global_slot);
    (epoch.saturating_add(1))
        .checked_mul(global_slot.slots_per_epoch.as_u32())
        .expect("overflow")
}

impl BlockWithoutProof {
    pub fn with_hash_and_proof(
        self,
        hash: v2::StateHash,
        proof: Arc<v2::MinaBaseProofStableV2>,
    ) -> ArcBlockWithHash {
        let block = v2::MinaBlockBlockStableV2 {
            header: v2::MinaBlockHeaderStableV2 {
                protocol_state: self.protocol_state,
                protocol_state_proof: proof,
                delta_block_chain_proof: self.delta_block_chain_proof,
                current_protocol_version: self.current_protocol_version,
                proposed_protocol_version_opt: self.proposed_protocol_version_opt,
            },
            body: self.body,
        };

        ArcBlockWithHash {
            block: block.into(),
            hash,
        }
    }
}

/// The epoch seed after a block in the seed update range
/// ([`mina_core::consensus::calc_epoch_seed`]).
pub fn calc_epoch_seed(
    prev_epoch_seed: &v2::EpochSeed,
    vrf_hash: mina_curves::pasta::Fp,
) -> v2::EpochSeed {
    // TODO(adonagy): fix this unwrap
    mina_core::consensus::calc_epoch_seed(prev_epoch_seed, vrf_hash).expect("invalid epoch seed")
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    use mina_node_account::AccountSecretKey;
    use mina_p2p_messages::bigint::BigInt;

    pub(super) fn vrf_won_slot(slot: u32) -> VrfWonSlotWithHash {
        let public_key = AccountSecretKey::genesis_producer().public_key();
        let vrf = mina_vrf::VrfWonSlot {
            producer: public_key.clone(),
            winner_account: public_key,
            vrf_output: Box::new(
                mina_vrf::genesis_vrf(v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(
                    BigInt::zero(),
                )))
                .unwrap(),
            ),
            global_slot: slot,
            account_index: AccountIndex(0),
            value_with_threshold: None,
        };
        VrfWonSlotWithHash::new(vrf, v2::LedgerHash::zero())
    }

    pub(super) fn won_slot(slot: u32) -> BlockProducerWonSlot {
        BlockProducerWonSlot::from_vrf_won_slot(&vrf_won_slot(slot), redux::Timestamp::ZERO)
    }

    #[test]
    fn mesa_slot_window_and_fork_offset() {
        // OCaml 4.0.2 node_config/profiled/mainnet.ml:11,43 (devnet includes it):
        // 7140 slots/epoch, 90000 ms/slot.
        let slot = won_slot(7140);
        assert_eq!(u64::from(slot.slot_time), 642_600_000_000_000);
        assert_eq!(u64::from(slot.next_slot_time()), 642_690_000_000_000);
        assert_eq!(slot.epoch(), 1);
        assert_eq!(slot.global_slot_since_genesis(0).as_u32(), 7140);
        for state in [
            BlockProducerCurrentState::WonSlot {
                time: redux::Timestamp::ZERO,
                won_slot: slot.clone(),
            },
            BlockProducerCurrentState::WonSlotWait {
                time: redux::Timestamp::ZERO,
                won_slot: slot.clone(),
            },
        ] {
            let start = u64::from(slot.slot_time);
            assert!(!state.won_slot_should_produce(redux::Timestamp::new(start - 21_000_000_000)));
            assert!(state.won_slot_should_produce(redux::Timestamp::new(start - 6_000_000_000)));
            assert!(state.won_slot_should_produce(redux::Timestamp::new(start + 89_999_999_999)));
            assert!(!state.won_slot_should_produce(redux::Timestamp::new(start + 90_000_000_000)));
            assert!(!state.won_slot_should_produce(redux::Timestamp::new(start + 179_000_000_000)));
        }
    }

    pub(super) fn pending_proof() -> BlockProducerEnabled {
        use crate::transition_frontier::genesis::GenesisConfigLoaded;
        let mut pending_coinbase = ledger::scan_state::pending_coinbase::PendingCoinbase::create(
            constraint_constants().pending_coinbase_depth,
        );
        pending_coinbase.merkle_root();
        let sparse = v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: List::new(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(v2::LedgerHash::zero()),
        };
        let amount = v2::CurrencyAmountStableV1(1_000_000_000u64.into());
        let loaded = GenesisConfigLoaded {
            runtime_config: Default::default(),
            constants: mina_core::constants::PROTOCOL_CONSTANTS.clone(),
            genesis_ledger_hash: v2::LedgerHash::zero(),
            genesis_total_currency: amount.clone(),
            genesis_producer_stake_proof: sparse.clone(),
            staking_epoch_ledger_hash: v2::LedgerHash::zero(),
            staking_epoch_total_currency: amount.clone(),
            staking_epoch_seed: v2::EpochSeed::zero(),
            next_epoch_ledger_hash: v2::LedgerHash::zero(),
            next_epoch_total_currency: amount.clone(),
            next_epoch_seed: v2::EpochSeed::zero(),
        };
        let (_, protocol_state, hash) = loaded
            .protocol_states(amount.clone(), amount.clone(), amount)
            .unwrap();
        let mut bp = BlockProducerState::new(
            redux::Timestamp::ZERO,
            Some(BlockProducerConfig::new(
                AccountSecretKey::genesis_producer().public_key().into(),
            )),
        );
        let bp = bp.as_mut().unwrap();
        bp.current = BlockProducerCurrentState::BlockProvePending {
            time: redux::Timestamp::ZERO,
            won_slot: won_slot(1),
            chain: vec![],
            emitted_ledger_proof: None,
            pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1::zero(),
            pending_coinbase_witness: v2::MinaBasePendingCoinbaseWitnessStableV2 {
                pending_coinbases: (&pending_coinbase).into(),
                is_new_stack: true,
            },
            stake_proof_sparse_ledger: sparse,
            block: BlockWithoutProof {
                protocol_state,
                delta_block_chain_proof: (v2::StateHash::zero(), List::new()),
                current_protocol_version: mina_core::constants::PROTOCOL_VERSION.clone(),
                proposed_protocol_version_opt: None,
                body: v2::StagedLedgerDiffBodyStableV1 {
                    staged_ledger_diff: (&ledger::staged_ledger::diff::with_valid_signatures_and_proofs::Diff::empty()).into(),
                },
            },
            block_hash: hash,
            verification_req_id: None,
        };
        bp.clone()
    }

    #[test]
    fn self_check_retries_identical_inputs_once_then_discards() {
        let mut bp = pending_proof();
        let original = bp.current.clone();
        let hash = match &original {
            BlockProducerCurrentState::BlockProvePending { block_hash, .. } => block_hash.clone(),
            _ => unreachable!(),
        };
        assert!(!bp.retry_failed_proof(&v2::StateHash::zero(), redux::Timestamp::ZERO));
        assert!(bp.retry_failed_proof(&hash, redux::Timestamp::ZERO));
        // The envelope changes; every input, including the block's parent,
        // ledger witness and pending coinbase, must remain byte-equivalent.
        let mut before = serde_json::to_value(&original).unwrap();
        before["BlockProvePending"]
            .as_object_mut()
            .unwrap()
            .remove("verification_req_id");
        let after = serde_json::to_value(&bp.current).unwrap();
        assert_eq!(before["BlockProvePending"], after["BlockUnprovenBuilt"]);
        bp.current = original;
        assert!(!bp.retry_failed_proof(&hash, redux::Timestamp::ZERO));
        assert!(matches!(bp.current, BlockProducerCurrentState::Idle { .. }));
        assert!(bp.current.produced_block().is_none());
    }

    #[test]
    fn self_check_does_not_retry_an_expired_slot() {
        let mut bp = pending_proof();
        let (hash, end) = match &bp.current {
            BlockProducerCurrentState::BlockProvePending {
                block_hash,
                won_slot,
                ..
            } => (block_hash.clone(), won_slot.next_slot_time()),
            _ => unreachable!(),
        };
        assert!(!bp.retry_failed_proof(&hash, end));
        assert!(matches!(bp.current, BlockProducerCurrentState::Idle { .. }));
    }
}

#[cfg(test)]
mod proof_tests;
