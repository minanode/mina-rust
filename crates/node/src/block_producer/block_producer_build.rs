//! Building a block from a won slot and a staged ledger diff, and the block
//! prover's input for it: what the block producer's reducer and effects do
//! between `StagedLedgerDiffCreateSuccess` and `BlockProveInit`, as functions
//! that tools can call too (`tools/block-replay/examples/mesa_offline_chain.rs`
//! proves blocks built with them).

use ledger::scan_state::currency::{Amount, Signed};
use mina_core::{
    block::AppliedBlock,
    consensus::{next_consensus_state, ConsensusConstants, ConsensusStateUpdate},
};
use mina_node_account::AccountSecretKey;
use mina_p2p_messages::{list::List, v2};

use super::{BlockProducerConfig, BlockProducerWonSlot, BlockWithoutProof};

/// The block a producer builds on top of `chain` for `won_slot`, with the
/// staged ledger diff it created, and the block's state hash.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L173-L436>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L1855-L1955>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L3236-L3290>
#[allow(clippy::too_many_arguments)]
pub fn build_unproven_block(
    consensus_constants: &ConsensusConstants,
    config: &BlockProducerConfig,
    won_slot: &BlockProducerWonSlot,
    chain: &[AppliedBlock],
    diff: &v2::StagedLedgerDiffDiffStableV2,
    diff_hash: &v2::ConsensusBodyReferenceStableV1,
    staged_ledger_hash: &v2::MinaBaseStagedLedgerHashStableV1,
    emitted_ledger_proof: Option<&v2::LedgerProofProdStableV2>,
    stake_proof_sparse_ledger: &v2::MinaBaseSparseLedgerBaseStableV2,
) -> Result<(BlockWithoutProof, v2::StateHash), String> {
    let Some(pred_block) = chain.last() else {
        return Err("no predecessor block".to_owned());
    };

    let pred_consensus_state = &pred_block.header().protocol_state.body.consensus_state;
    let pred_blockchain_state = &pred_block.header().protocol_state.body.blockchain_state;

    let genesis_ledger_hash = &pred_blockchain_state.genesis_ledger_hash;

    let block_timestamp = won_slot.timestamp();
    let global_slot_since_genesis =
        won_slot.global_slot_since_genesis(pred_block.global_slot_diff());

    let block_stake_winner = won_slot.delegator.0.clone();

    let ledger_proof_statement = ledger_proof_statement_from_emitted_proof(
        emitted_ledger_proof,
        &pred_blockchain_state.ledger_proof_statement,
    );

    let supply_increase = emitted_ledger_proof.map_or(Signed::zero(), |v| {
        Signed::from(&v.statement.supply_increase)
    });

    let total_currency = {
        let (amount, overflowed) = Amount::from(pred_consensus_state.total_currency.clone())
            .add_signed_flagged(supply_increase);
        if overflowed {
            return Err("total_currency overflowed".to_owned());
        }
        amount
    };

    let supercharge_coinbase = can_apply_supercharged_coinbase(
        &block_stake_winner,
        stake_proof_sparse_ledger,
        &global_slot_since_genesis,
    );
    let consensus_state = next_consensus_state(
        consensus_constants,
        pred_consensus_state,
        pred_block.hash(),
        ConsensusStateUpdate {
            curr_global_slot_since_hard_fork: won_slot.global_slot.clone(),
            vrf_hash: won_slot.vrf_output.hash(),
            last_vrf_output: (*won_slot.vrf_output).clone().into(),
            snarked_ledger_hash: pred_block.snarked_ledger_hash().clone(),
            genesis_ledger_hash: genesis_ledger_hash.clone(),
            total_currency: (&total_currency).into(),
            block_stake_winner,
            block_creator: config.pub_key.clone(),
            coinbase_receiver: config.coinbase_receiver().clone(),
            supercharge_coinbase,
        },
    )?;
    let proposed_protocol_version_opt = config.proposed_protocol_version.clone();

    let protocol_state = v2::MinaStateProtocolStateValueStableV2 {
        previous_state_hash: pred_block.hash().clone(),
        body: v2::MinaStateProtocolStateBodyValueStableV2 {
            genesis_state_hash: if pred_block.is_genesis() {
                pred_block.hash().clone()
            } else {
                pred_block
                    .header()
                    .protocol_state
                    .body
                    .genesis_state_hash
                    .clone()
            },
            constants: pred_block.header().protocol_state.body.constants.clone(),
            blockchain_state: v2::MinaStateBlockchainStateValueStableV2 {
                staged_ledger_hash: staged_ledger_hash.clone(),
                genesis_ledger_hash: genesis_ledger_hash.clone(),
                ledger_proof_statement,
                timestamp: block_timestamp,
                body_reference: diff_hash.clone(),
            },
            consensus_state,
        },
    };

    let chain_proof_len = pred_block.constants().delta.as_u32() as usize;
    let delta_block_chain_proof = match chain_proof_len {
        0 => (pred_block.hash().clone(), List::new()),
        chain_proof_len => {
            // TODO(binier): test
            let mut iter = chain
                .iter()
                .rev()
                .take(chain_proof_len.saturating_add(1))
                .rev();
            if let Some(first_block) = iter.next() {
                let first_hash = first_block.hash().clone();
                let body_hashes = iter
                    .filter_map(|b| b.header().protocol_state.body.try_hash().ok()) // TODO: Handle error ?
                    .map(v2::StateBodyHash::from)
                    .collect();
                (first_hash, body_hashes)
            } else {
                // TODO: test this as well
                // If the chain is empty, return the same as when chain_proof_len is 0
                (pred_block.hash().clone(), List::new())
            }
        }
    };

    let block = BlockWithoutProof {
        protocol_state,
        delta_block_chain_proof,
        current_protocol_version: pred_block.header().current_protocol_version.clone(),
        proposed_protocol_version_opt,
        body: v2::StagedLedgerDiffBodyStableV1 {
            staged_ledger_diff: diff.clone(),
        },
    };
    let block_hash = block
        .protocol_state
        .try_hash()
        .map_err(|_| "Invalid protocol state".to_owned())?;
    Ok((block, block_hash))
}

/// The block prover's input for `block`, built on `chain` for `won_slot`: the
/// previous state and its proof, the new state, the snark transition, the
/// ledger proof the staged ledger emitted, the stake proof and the pending
/// coinbase witness.
///
/// The producer's private key is a placeholder, the genesis winner's: the
/// prover service puts the producer's in (`BlockProducerService::prove`).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/prover/prover.ml#L518-L569>
#[allow(clippy::too_many_arguments)]
pub fn block_prover_input(
    won_slot: &BlockProducerWonSlot,
    chain: &[AppliedBlock],
    emitted_ledger_proof: Option<std::sync::Arc<v2::LedgerProofProdStableV2>>,
    pending_coinbase_update: &v2::MinaBasePendingCoinbaseUpdateStableV1,
    pending_coinbase_witness: &v2::MinaBasePendingCoinbaseWitnessStableV2,
    stake_proof_sparse_ledger: &v2::MinaBaseSparseLedgerBaseStableV2,
    block: &BlockWithoutProof,
) -> Option<v2::ProverExtendBlockchainInputStableV2> {
    let pred_block = chain.last()?;
    let consensus_state = &block.protocol_state.body.consensus_state;

    Some(v2::ProverExtendBlockchainInputStableV2 {
        chain: v2::BlockchainSnarkBlockchainStableV2 {
            state: pred_block.header().protocol_state.clone(),
            proof: pred_block.header().protocol_state_proof.clone(),
        },
        next_state: block.protocol_state.clone(),
        block: v2::MinaStateSnarkTransitionValueStableV2 {
            blockchain_state: block.protocol_state.body.blockchain_state.clone(),
            consensus_transition: consensus_state
                .curr_global_slot_since_hard_fork
                .slot_number
                .clone(),
            pending_coinbase_update: pending_coinbase_update.clone(),
        },
        ledger_proof: emitted_ledger_proof,
        prover_state: v2::ConsensusStakeProofStableV2 {
            delegator: won_slot.delegator.1.into(),
            delegator_pk: won_slot.delegator.0.clone(),
            coinbase_receiver_pk: consensus_state.coinbase_receiver.clone(),
            ledger: stake_proof_sparse_ledger.clone(),
            // it is replaced with correct keys in the service.
            producer_private_key: AccountSecretKey::genesis_producer().into(),
            producer_public_key: consensus_state.block_creator.clone(),
        },
        pending_coinbase: pending_coinbase_witness.clone(),
    })
}

fn can_apply_supercharged_coinbase(
    block_stake_winner: &v2::NonZeroCurvePoint,
    stake_proof_sparse_ledger: &v2::MinaBaseSparseLedgerBaseStableV2,
    global_slot_since_genesis: &v2::MinaNumbersGlobalSlotSinceGenesisMStableV1,
) -> bool {
    use ledger::staged_ledger::staged_ledger::StagedLedger;

    let winner = (block_stake_winner)
        .try_into()
        .expect("Public key being used cannot be invalid here");
    let epoch_ledger = (stake_proof_sparse_ledger)
        .try_into()
        .expect("Sparse ledger being used cannot be invalid here");
    let global_slot = (global_slot_since_genesis).into();

    StagedLedger::can_apply_supercharged_coinbase_exn(winner, &epoch_ledger, global_slot)
}

fn ledger_proof_statement_from_emitted_proof(
    emitted_ledger_proof: Option<&v2::LedgerProofProdStableV2>,
    pred_proof_statement: &v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
) -> v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement {
    match emitted_ledger_proof.map(|proof| &proof.statement) {
        None => pred_proof_statement.clone(),
        Some(stmt) => v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement {
            source: stmt.source.clone(),
            target: stmt.target.clone(),
            connecting_ledger_left: stmt.connecting_ledger_left.clone(),
            connecting_ledger_right: stmt.connecting_ledger_right.clone(),
            supply_increase: stmt.supply_increase.clone(),
            fee_excess: stmt.fee_excess.clone(),
            sok_digest: (),
        },
    }
}
