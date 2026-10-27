use crate::{
    p2p::P2pInitializeAction,
    transition_frontier::genesis_effectful::TransitionFrontierGenesisEffectfulAction,
};
use ledger::dummy::dummy_blockchain_proof;
use mina_core::{block::BlockWithHash, constants::PROTOCOL_VERSION, error, info};
use mina_p2p_messages::v2;

use super::{
    empty_block_body, genesis_block_prover_input, TransitionFrontierGenesisAction,
    TransitionFrontierGenesisActionWithMetaRef, TransitionFrontierGenesisState,
};

impl TransitionFrontierGenesisState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: TransitionFrontierGenesisActionWithMetaRef<'_>,
        skip_proof_verification: bool,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            // TODO: log or propagate
            return;
        };
        let (action, meta) = action.split();

        match action {
            TransitionFrontierGenesisAction::LedgerLoadInit => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let config = global_state.transition_frontier.config.genesis.clone();

                dispatcher.push(TransitionFrontierGenesisAction::LedgerLoadPending);
                dispatcher
                    .push(TransitionFrontierGenesisEffectfulAction::LedgerLoadInit { config });
            }
            TransitionFrontierGenesisAction::LedgerLoadPending => {
                *state = Self::LedgerLoadPending { time: meta.time() };
            }
            TransitionFrontierGenesisAction::LedgerLoadSuccess { data } => {
                *state = Self::LedgerLoadSuccess {
                    time: meta.time(),
                    data: *data.clone(),
                };

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                // TODO(refactor): before this is dispatched genesis inject must be dispatched
                dispatcher.push(TransitionFrontierGenesisAction::Produce);
            }
            TransitionFrontierGenesisAction::Produce => {
                let Self::LedgerLoadSuccess { data, .. } = state else {
                    return;
                };

                let protocol_states =
                    |genesis_total_currency, staking_total_currency, next_total_currency| {
                        data.protocol_states(
                            genesis_total_currency,
                            staking_total_currency,
                            next_total_currency,
                        )
                    };

                let Ok((negative_one, genesis, genesis_hash)) = protocol_states(
                    data.genesis_total_currency.clone(),
                    data.staking_epoch_total_currency.clone(),
                    data.next_epoch_total_currency.clone(),
                ) else {
                    error!(meta.time(); "invalid negative protocol state");
                    return;
                };
                // The chain id commits to the genesis state with zero total
                // currencies, so it does not depend on ledger contents.
                //
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/chain_id/chain_id.ml#L44-L70>
                let zero = || v2::CurrencyAmountStableV1(0u64.into());
                let Ok((_, _, chain_id_genesis_hash)) = protocol_states(zero(), zero(), zero())
                else {
                    error!(meta.time(); "invalid negative protocol state");
                    return;
                };

                // Built once here rather than on every `genesis_block()`
                // call: it hashes the protocol state (I20).
                let Ok(block_with_dummy_proof) =
                    Self::build_block_with_dummy_proof(&genesis, &genesis_hash)
                else {
                    error!(meta.time(); "invalid genesis block");
                    return;
                };

                *state = Self::Produced {
                    time: meta.time(),
                    negative_one,
                    genesis,
                    genesis_hash,
                    chain_id_genesis_hash,
                    genesis_producer_stake_proof: data.genesis_producer_stake_proof.clone(),
                    block_with_dummy_proof,
                };

                // Dispatch
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                if global_state.p2p.ready().is_none() {
                    let TransitionFrontierGenesisState::Produced {
                        genesis,
                        chain_id_genesis_hash,
                        ..
                    } = &global_state.transition_frontier.genesis
                    else {
                        error!(meta.time(); "incorrect state: {:?}", global_state.transition_frontier.genesis);
                        return;
                    };
                    use mina_core::{constants, ChainId};
                    let chain_id = if let Some(override_id) = &global_state.config.chain_id_override
                    {
                        override_id.clone()
                    } else {
                        let constraint_system_digests =
                            mina_core::network::constraint_system_digests();
                        ChainId::compute(
                            constraint_system_digests,
                            chain_id_genesis_hash,
                            &genesis.body.constants,
                            constants::PROTOCOL_TRANSACTION_VERSION,
                            constants::PROTOCOL_NETWORK_VERSION,
                            &v2::UnsignedExtendedUInt32StableV1::from(constants::TX_POOL_MAX_SIZE),
                        )
                    };
                    info!(meta.time();
                        summary = "chain id",
                        chain_id = chain_id.to_hex(),
                        overridden = global_state.config.chain_id_override.is_some(),
                        genesis_hash = chain_id_genesis_hash.to_string(),
                    );
                    dispatcher.push(P2pInitializeAction::Initialize { chain_id });
                }
                dispatcher.push(TransitionFrontierGenesisAction::ProveInit);
            }
            TransitionFrontierGenesisAction::ProveInit => {
                let TransitionFrontierGenesisState::Produced {
                    negative_one,
                    genesis,
                    genesis_hash,
                    genesis_producer_stake_proof,
                    ..
                } = state
                else {
                    return;
                };

                let genesis_hash = genesis_hash.clone();
                let input =
                    genesis_block_prover_input(negative_one, genesis, genesis_producer_stake_proof);
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(won_slot) = global_state.block_producer.current_won_slot() else {
                    return;
                };
                let deadline = won_slot.next_slot_time();
                dispatcher.push(TransitionFrontierGenesisAction::ProvePending { deadline });
                if skip_proof_verification {
                    dispatcher.push(TransitionFrontierGenesisAction::ProveSuccess {
                        proof: dummy_blockchain_proof().clone(),
                    });
                } else {
                    dispatcher.push(TransitionFrontierGenesisEffectfulAction::ProveInit {
                        block_hash: genesis_hash,
                        input: input.into(),
                        deadline,
                    });
                }
            }
            TransitionFrontierGenesisAction::ProvePending { deadline } => {
                let Self::Produced {
                    negative_one,
                    genesis,
                    genesis_hash,
                    genesis_producer_stake_proof,
                    block_with_dummy_proof,
                    ..
                } = state
                else {
                    return;
                };

                *state = Self::ProvePending {
                    time: meta.time(),
                    negative_one: negative_one.clone(),
                    genesis: genesis.clone(),
                    genesis_hash: genesis_hash.clone(),
                    genesis_producer_stake_proof: genesis_producer_stake_proof.clone(),
                    block_with_dummy_proof: block_with_dummy_proof.clone(),
                    deadline: *deadline,
                    attempt: 1,
                };
            }
            TransitionFrontierGenesisAction::ProveError { error } => {
                let Self::ProvePending {
                    negative_one,
                    genesis,
                    genesis_hash,
                    genesis_producer_stake_proof,
                    block_with_dummy_proof,
                    deadline,
                    attempt,
                    ..
                } = state
                else {
                    return;
                };
                if *attempt < 2 && meta.time() < *deadline {
                    *attempt += 1;
                    let input = genesis_block_prover_input(
                        negative_one,
                        genesis,
                        genesis_producer_stake_proof,
                    );
                    let action = TransitionFrontierGenesisEffectfulAction::ProveInit {
                        block_hash: genesis_hash.clone(),
                        input: input.into(),
                        deadline: *deadline,
                    };
                    info!(meta.time(); message = "Genesis proof failed; retrying once", error = error);
                    state_context.into_dispatcher().push(action);
                } else {
                    error!(meta.time(); message = "Genesis proof failed; giving up", attempts = *attempt, error = error);
                    *state = Self::ProveError {
                        time: meta.time(),
                        block_with_dummy_proof: block_with_dummy_proof.clone(),
                        error: error.clone(),
                        attempts: *attempt,
                    };
                }
            }
            TransitionFrontierGenesisAction::ProveSuccess { proof } => {
                let Self::ProvePending {
                    genesis,
                    genesis_hash,
                    ..
                } = state
                else {
                    return;
                };

                let block = v2::MinaBlockBlockStableV2 {
                    header: v2::MinaBlockHeaderStableV2 {
                        protocol_state: genesis.clone(),
                        protocol_state_proof: proof.clone(),
                        delta_block_chain_proof: (
                            genesis_hash.clone(),
                            std::iter::empty().collect(),
                        ),
                        current_protocol_version: PROTOCOL_VERSION.clone(),
                        proposed_protocol_version_opt: None,
                    },
                    body: v2::StagedLedgerDiffBodyStableV1 {
                        staged_ledger_diff: empty_block_body(),
                    },
                };

                let Ok(genesis) = BlockWithHash::try_new(block.into()) else {
                    error!(meta.time(); "invalid `genesis` block");
                    return;
                };

                *state = Self::ProveSuccess {
                    time: meta.time(),
                    genesis,
                };
            }
        }
    }
}
