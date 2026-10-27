mod transition_frontier_genesis_config;
pub use transition_frontier_genesis_config::*;

mod transition_frontier_genesis_state;
pub use transition_frontier_genesis_state::*;

mod transition_frontier_genesis_actions;
pub use transition_frontier_genesis_actions::*;

mod transition_frontier_genesis_reducer;

mod genesis_cache;
pub use genesis_cache::*;
#[cfg(test)]
mod genesis_cache_tests;

use ledger::scan_state::{
    pending_coinbase::PendingCoinbase, transaction_logic::local_state::LocalState,
};
use mina_core::{
    block::genesis::genesis_and_negative_one_protocol_states, constants::constraint_constants,
};
use mina_p2p_messages::{bigint::InvalidBigInt, v2};

use crate::{account::AccountSecretKey, block_producer::calc_epoch_seed};

impl GenesisConfigLoaded {
    /// The protocol states before and at genesis, and the genesis state hash,
    /// for the given total currencies.
    pub fn protocol_states(
        &self,
        genesis_total_currency: v2::CurrencyAmountStableV1,
        staking_epoch_total_currency: v2::CurrencyAmountStableV1,
        next_epoch_total_currency: v2::CurrencyAmountStableV1,
    ) -> Result<
        (
            v2::MinaStateProtocolStateValueStableV2,
            v2::MinaStateProtocolStateValueStableV2,
            v2::StateHash,
        ),
        InvalidBigInt,
    > {
        let genesis_vrf = ::mina_vrf::genesis_vrf(self.staking_epoch_seed.clone()).unwrap();
        let genesis_vrf_hash = genesis_vrf.hash();
        genesis_and_negative_one_protocol_states(
            self.constants.clone(),
            self.genesis_ledger_hash.clone(),
            genesis_total_currency,
            self.staking_epoch_ledger_hash.clone(),
            staking_epoch_total_currency,
            self.next_epoch_ledger_hash.clone(),
            next_epoch_total_currency,
            AccountSecretKey::genesis_producer().public_key().into(),
            empty_pending_coinbase_hash(),
            (&LocalState::dummy()).into(),
            empty_block_body_hash(),
            genesis_vrf.into(),
            self.staking_epoch_seed.clone(),
            self.next_epoch_seed.clone(),
            calc_epoch_seed(&self.next_epoch_seed, genesis_vrf_hash),
        )
    }
}

/// The block prover's input for the genesis block, the base case of the
/// blockchain step: the negative-one state with a dummy proof as the chain,
/// the genesis transition, no ledger proof, the genesis winner's stake proof
/// (its account in the staking epoch ledger, `genesis_producer_stake_proof`)
/// with its private key, and the empty pending coinbase.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/prover/prover.ml#L571-L617>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L772-L787>
pub fn genesis_block_prover_input(
    negative_one: &v2::MinaStateProtocolStateValueStableV2,
    genesis: &v2::MinaStateProtocolStateValueStableV2,
    genesis_producer_stake_proof: &v2::MinaBaseSparseLedgerBaseStableV2,
) -> v2::ProverExtendBlockchainInputStableV2 {
    let consensus_state = &genesis.body.consensus_state;
    v2::ProverExtendBlockchainInputStableV2 {
        chain: v2::BlockchainSnarkBlockchainStableV2 {
            state: negative_one.clone(),
            proof: ledger::dummy::dummy_blockchain_proof().clone(),
        },
        next_state: genesis.clone(),
        block: v2::MinaStateSnarkTransitionValueStableV2 {
            blockchain_state: genesis.body.blockchain_state.clone(),
            consensus_transition: consensus_state
                .curr_global_slot_since_hard_fork
                .slot_number
                .clone(),
            pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1::zero(),
        },
        ledger_proof: None,
        prover_state: v2::ConsensusStakeProofStableV2 {
            delegator: v2::MinaBaseAccountIndexStableV1(0u64.into()),
            delegator_pk: consensus_state.block_stake_winner.clone(),
            coinbase_receiver_pk: consensus_state.coinbase_receiver.clone(),
            producer_public_key: consensus_state.block_creator.clone(),
            producer_private_key: AccountSecretKey::genesis_producer().into(),
            ledger: genesis_producer_stake_proof.clone(),
        },
        pending_coinbase: v2::MinaBasePendingCoinbaseWitnessStableV2 {
            pending_coinbases: (&empty_pending_coinbase()).into(),
            is_new_stack: true,
        },
    }
}

pub(super) fn empty_block_body() -> v2::StagedLedgerDiffDiffStableV2 {
    use ledger::staged_ledger::diff::with_valid_signatures_and_proofs::Diff;
    (&Diff::empty()).into()
}

pub(super) fn empty_block_body_hash() -> v2::ConsensusBodyReferenceStableV1 {
    use ledger::staged_ledger::validate_block::block_body_hash;
    block_body_hash(&empty_block_body()).unwrap()
}

pub(super) fn empty_pending_coinbase() -> PendingCoinbase {
    let mut v = PendingCoinbase::create(constraint_constants().pending_coinbase_depth);
    v.merkle_root();
    v
}

pub fn empty_pending_coinbase_hash() -> v2::PendingCoinbaseHash {
    v2::MinaBasePendingCoinbaseHashVersionedStableV1(
        v2::MinaBasePendingCoinbaseHashBuilderStableV1(
            empty_pending_coinbase().merkle_root().into(),
        ),
    )
    .into()
}
