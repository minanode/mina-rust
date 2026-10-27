use ledger::dummy::dummy_blockchain_proof;
use mina_core::{block::ArcBlockWithHash, constants::PROTOCOL_VERSION};
use mina_p2p_messages::{bigint::InvalidBigInt, v2};
use serde::{Deserialize, Serialize};

use super::{empty_block_body, GenesisConfigLoaded};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TransitionFrontierGenesisState {
    Idle,
    LedgerLoadPending {
        time: redux::Timestamp,
    },
    LedgerLoadSuccess {
        time: redux::Timestamp,
        data: GenesisConfigLoaded,
    },
    Produced {
        time: redux::Timestamp,
        negative_one: v2::MinaStateProtocolStateValueStableV2,
        genesis: v2::MinaStateProtocolStateValueStableV2,
        genesis_hash: v2::StateHash,
        /// Hash of the genesis state with all total currencies set to zero,
        /// which the chain id commits to instead of `genesis_hash`.
        chain_id_genesis_hash: v2::StateHash,
        genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2,
        /// `genesis` as a block with a dummy proof, built once on entering
        /// this state.
        ///
        /// A node that doesn't produce blocks stays in this state, and
        /// [`crate::State::genesis_block`] returns this block. The
        /// transaction pool calls that twice per action, for the current
        /// global slot, so each `CheckTimeoutsAction` called it at least four
        /// times. Building the block on each call hashed the protocol state,
        /// about 0.8 ms natively, and was over 90% of the cost of
        /// `CheckTimeoutsAction` (I20).
        block_with_dummy_proof: ArcBlockWithHash,
    },
    ProvePending {
        time: redux::Timestamp,
        negative_one: v2::MinaStateProtocolStateValueStableV2,
        genesis: v2::MinaStateProtocolStateValueStableV2,
        genesis_hash: v2::StateHash,
        genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2,
        block_with_dummy_proof: ArcBlockWithHash,
        /// Both attempts share the end of the original won slot.
        deadline: redux::Timestamp,
        attempt: u8,
    },
    /// No proof is in flight. Sync/prevalidation keep their genesis context,
    /// but production on this unproved genesis is disabled until restart.
    ProveError {
        time: redux::Timestamp,
        block_with_dummy_proof: ArcBlockWithHash,
        error: String,
        attempts: u8,
    },
    ProveSuccess {
        time: redux::Timestamp,
        genesis: ArcBlockWithHash,
    },
}

impl TransitionFrontierGenesisState {
    /// Build the genesis block with a dummy proof, as stored in
    /// [`Self::Produced`]. This hashes the protocol state.
    pub fn build_block_with_dummy_proof(
        genesis: &v2::MinaStateProtocolStateValueStableV2,
        genesis_hash: &v2::StateHash,
    ) -> Result<ArcBlockWithHash, InvalidBigInt> {
        ArcBlockWithHash::try_new(
            v2::MinaBlockBlockStableV2 {
                header: v2::MinaBlockHeaderStableV2 {
                    protocol_state: genesis.clone(),
                    protocol_state_proof: dummy_blockchain_proof().clone(),
                    delta_block_chain_proof: (genesis_hash.clone(), std::iter::empty().collect()),
                    current_protocol_version: PROTOCOL_VERSION.clone(),
                    proposed_protocol_version_opt: None,
                },
                body: v2::StagedLedgerDiffBodyStableV1 {
                    staged_ledger_diff: empty_block_body(),
                },
            }
            .into(),
        )
    }

    /// The genesis block while unproved, including pending and failed proving.
    /// Cheap: the block is built once, when the state is entered.
    pub fn block_with_dummy_proof(&self) -> Option<ArcBlockWithHash> {
        match self {
            Self::Produced {
                block_with_dummy_proof,
                ..
            }
            | Self::ProvePending {
                block_with_dummy_proof,
                ..
            }
            | Self::ProveError {
                block_with_dummy_proof,
                ..
            } => Some(block_with_dummy_proof.clone()),
            _ => None,
        }
    }

    pub fn prove_pending_block_hash(&self) -> Option<v2::StateHash> {
        match self {
            Self::ProvePending { genesis_hash, .. } => Some(genesis_hash.clone()),
            _ => None,
        }
    }

    pub fn proven_block(&self) -> Option<&ArcBlockWithHash> {
        match self {
            Self::ProveSuccess { genesis, .. } => Some(genesis),
            _ => None,
        }
    }

    pub fn block_with_real_or_dummy_proof(&self) -> Option<ArcBlockWithHash> {
        self.proven_block()
            .cloned()
            .or_else(|| self.block_with_dummy_proof())
    }
}

#[cfg(test)]
impl TransitionFrontierGenesisState {
    /// A `Produced` state for a genesis config with made-up ledger hashes,
    /// currencies and seeds: the values don't matter here.
    pub(crate) fn produced_for_tests() -> Self {
        let ledger_hash: v2::LedgerHash = "jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS"
            .parse()
            .unwrap();
        let seed: v2::EpochSeed = "2vamERQZWT1miAnrxLJsjPmZmoCSemrz9JugjN8weQJeqhU7ZD38"
            .parse()
            .unwrap();
        let loaded = GenesisConfigLoaded {
            runtime_config: Default::default(),
            constants: mina_core::constants::PROTOCOL_CONSTANTS.clone(),
            genesis_ledger_hash: ledger_hash.clone(),
            genesis_total_currency: test_amount(1_000_000_000),
            genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2 {
                indexes: std::iter::empty().collect(),
                depth: 35u64.into(),
                tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(ledger_hash.clone()),
            },
            staking_epoch_ledger_hash: ledger_hash.clone(),
            staking_epoch_total_currency: test_amount(2_000_000_000),
            staking_epoch_seed: seed.clone(),
            next_epoch_ledger_hash: ledger_hash,
            next_epoch_total_currency: test_amount(3_000_000_000),
            next_epoch_seed: seed,
        };
        let (negative_one, genesis, genesis_hash) = loaded
            .protocol_states(
                loaded.genesis_total_currency.clone(),
                loaded.staking_epoch_total_currency.clone(),
                loaded.next_epoch_total_currency.clone(),
            )
            .unwrap();
        let block_with_dummy_proof =
            Self::build_block_with_dummy_proof(&genesis, &genesis_hash).unwrap();
        Self::Produced {
            time: redux::Timestamp::ZERO,
            negative_one,
            genesis,
            chain_id_genesis_hash: genesis_hash.clone(),
            genesis_hash,
            genesis_producer_stake_proof: loaded.genesis_producer_stake_proof,
            block_with_dummy_proof,
        }
    }
}

#[cfg(test)]
fn test_amount(nanomina: u64) -> v2::CurrencyAmountStableV1 {
    v2::CurrencyAmountStableV1(nanomina.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{dummy_blockchain_proof, empty_block_body, TransitionFrontierGenesisState};

    fn produced() -> TransitionFrontierGenesisState {
        TransitionFrontierGenesisState::produced_for_tests()
    }

    #[test]
    fn block_with_dummy_proof_is_the_genesis_block() {
        let state = produced();
        let TransitionFrontierGenesisState::Produced {
            genesis,
            genesis_hash,
            ..
        } = &state
        else {
            unreachable!()
        };
        let block = state.block_with_dummy_proof().unwrap();

        // The hash is the protocol state's, computed when the block was built.
        assert_eq!(block.hash(), genesis_hash);
        assert_eq!(&genesis.try_hash().unwrap(), genesis_hash);

        let header = block.header();
        assert_eq!(&header.protocol_state, genesis);
        assert!(Arc::ptr_eq(
            &header.protocol_state_proof,
            &dummy_blockchain_proof()
        ));
        assert_eq!(&header.delta_block_chain_proof.0, genesis_hash);
        assert!(header.delta_block_chain_proof.1.is_empty());
        assert_eq!(block.body().staged_ledger_diff, empty_block_body());
    }

    #[test]
    fn block_with_dummy_proof_is_not_rebuilt() {
        let state = produced();
        let a = state.block_with_dummy_proof().unwrap();
        let b = state.block_with_dummy_proof().unwrap();
        let c = state.block_with_real_or_dummy_proof().unwrap();
        assert!(Arc::ptr_eq(&a.block, &b.block));
        assert!(Arc::ptr_eq(&a.block, &c.block));
    }

    #[test]
    fn block_with_dummy_proof_is_retained_while_proving() {
        assert!(TransitionFrontierGenesisState::Idle
            .block_with_dummy_proof()
            .is_none());
        let TransitionFrontierGenesisState::Produced {
            negative_one,
            genesis,
            genesis_hash,
            genesis_producer_stake_proof,
            ..
        } = produced()
        else {
            unreachable!()
        };
        let block_with_dummy_proof =
            TransitionFrontierGenesisState::build_block_with_dummy_proof(&genesis, &genesis_hash)
                .unwrap();
        let pending = TransitionFrontierGenesisState::ProvePending {
            time: redux::Timestamp::ZERO,
            negative_one,
            genesis,
            genesis_hash,
            genesis_producer_stake_proof,
            block_with_dummy_proof,
            deadline: redux::Timestamp::ZERO,
            attempt: 1,
        };
        assert!(pending.block_with_dummy_proof().is_some());
        assert!(pending.block_with_real_or_dummy_proof().is_some());
    }

    #[test]
    fn produced_state_round_trips_with_its_block() {
        let state = produced();
        let json = serde_json::to_string(&state).unwrap();
        let back: TransitionFrontierGenesisState = serde_json::from_str(&json).unwrap();
        let block = state.block_with_dummy_proof().unwrap();
        let back_block = back.block_with_dummy_proof().unwrap();
        assert_eq!(back_block.hash(), block.hash());
        assert_eq!(back_block.header(), block.header());
    }
}
