//! Protocol-state construction shared by the producer and incoming validation.

use ledger::scan_state::currency::{Amount, Signed};
use mina_core::consensus::{next_consensus_state, ConsensusConstants, ConsensusStateUpdate};
use mina_p2p_messages::v2;

type State = v2::MinaStateProtocolStateValueStableV2;
type Statement = v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement;

/// Inputs known only to the producer (or already checked by the block proof).
pub struct ProtocolStateInputs {
    pub slot: v2::MinaNumbersGlobalSlotSinceHardForkMStableV1,
    pub vrf_hash: mina_curves::pasta::Fp,
    pub last_vrf_output: v2::ConsensusVrfOutputTruncatedStableV1,
    pub block_stake_winner: v2::NonZeroCurvePoint,
    pub block_creator: v2::NonZeroCurvePoint,
    pub coinbase_receiver: v2::NonZeroCurvePoint,
    pub supercharge_coinbase: bool,
    pub timestamp: v2::BlockTimeTimeStableV1,
}

/// Build the full state from the parent and the staged-ledger application.
/// A missing emitted proof retains the parent's statement but adds zero supply.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L3260-L3298>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_block/validation.ml#L500-L522>
pub fn next_protocol_state(
    constants: &ConsensusConstants,
    parent: &State,
    parent_hash: &v2::StateHash,
    inputs: ProtocolStateInputs,
    staged_ledger_hash: &v2::MinaBaseStagedLedgerHashStableV1,
    body_reference: &v2::ConsensusBodyReferenceStableV1,
    emitted_statement: Option<&Statement>,
) -> Result<State, String> {
    let prev = &parent.body;
    let supply_increase =
        emitted_statement.map_or(Signed::zero(), |stmt| Signed::from(&stmt.supply_increase));
    let (total_currency, overflowed) = Amount::from(prev.consensus_state.total_currency.clone())
        .add_signed_flagged(supply_increase);
    if overflowed {
        return Err("total_currency overflowed".to_owned());
    }
    let consensus_state = next_consensus_state(
        constants,
        &prev.consensus_state,
        parent_hash,
        ConsensusStateUpdate {
            curr_global_slot_since_hard_fork: v2::ConsensusGlobalSlotStableV1 {
                slot_number: inputs.slot,
                slots_per_epoch: constants.slots_per_epoch.into(),
            },
            vrf_hash: inputs.vrf_hash,
            last_vrf_output: inputs.last_vrf_output,
            snarked_ledger_hash: prev
                .blockchain_state
                .ledger_proof_statement
                .target
                .first_pass_ledger
                .clone(),
            genesis_ledger_hash: prev.blockchain_state.genesis_ledger_hash.clone(),
            total_currency: (&total_currency).into(),
            block_stake_winner: inputs.block_stake_winner,
            block_creator: inputs.block_creator,
            coinbase_receiver: inputs.coinbase_receiver,
            supercharge_coinbase: inputs.supercharge_coinbase,
        },
    )?;
    Ok(State {
        previous_state_hash: parent_hash.clone(),
        body: v2::MinaStateProtocolStateBodyValueStableV2 {
            // OCaml's is_genesis_state tests the slot since the hard fork.
            genesis_state_hash: if prev
                .consensus_state
                .curr_global_slot_since_hard_fork
                .slot_number
                .as_u32()
                == 0
            {
                parent_hash.clone()
            } else {
                prev.genesis_state_hash.clone()
            },
            constants: prev.constants.clone(),
            blockchain_state: v2::MinaStateBlockchainStateValueStableV2 {
                staged_ledger_hash: staged_ledger_hash.clone(),
                genesis_ledger_hash: prev.blockchain_state.genesis_ledger_hash.clone(),
                ledger_proof_statement: emitted_statement
                    .unwrap_or(&prev.blockchain_state.ledger_proof_statement)
                    .clone(),
                timestamp: inputs.timestamp,
                body_reference: body_reference.clone(),
            },
            consensus_state,
        },
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Incorrect_protocol_state: {0}")]
pub struct IncorrectProtocolState(pub String);

/// Compare the entire received state with a rebuild after ledger application.
/// The timestamp is deliberately unconstrained. The full VRF output is absent
/// from the block: retain its truncated output and, only inside the seed-update
/// range, its next epoch seed. Stake winner, creator, receiver and supercharge
/// are producer inputs too. Everything else must equal the rebuilt state.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L3328-L3376>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_block/validation.ml#L500-L541>
pub fn check_protocol_state(
    constants: &ConsensusConstants,
    parent: &State,
    parent_hash: &v2::StateHash,
    candidate: &State,
    staged_ledger_hash: &v2::MinaBaseStagedLedgerHashStableV1,
    body_reference: &v2::ConsensusBodyReferenceStableV1,
    emitted_statement: Option<&Statement>,
) -> Result<(), IncorrectProtocolState> {
    let cs = &candidate.body.consensus_state;
    let mut expected = next_protocol_state(
        constants,
        parent,
        parent_hash,
        ProtocolStateInputs {
            slot: cs.curr_global_slot_since_hard_fork.slot_number.clone(),
            vrf_hash: mina_curves::pasta::Fp::from(0),
            last_vrf_output: cs.last_vrf_output.clone(),
            block_stake_winner: cs.block_stake_winner.clone(),
            block_creator: cs.block_creator.clone(),
            coinbase_receiver: cs.coinbase_receiver.clone(),
            supercharge_coinbase: cs.supercharge_coinbase,
            timestamp: candidate.body.blockchain_state.timestamp,
        },
        staged_ledger_hash,
        body_reference,
        emitted_statement,
    )
    .map_err(IncorrectProtocolState)?;
    // Use the candidate's epoch width here, as OCaml's adjustment does; the
    // full equality below still rejects an incorrect slots_per_epoch field.
    let epoch_width = cs.curr_global_slot_since_hard_fork.slots_per_epoch.as_u32();
    let slot = cs.curr_global_slot_since_hard_fork.slot_number.as_u32();
    if epoch_width != 0 && slot % epoch_width < constants.slots_per_epoch / 3 * 2 {
        expected.body.consensus_state.next_epoch_data.seed = cs.next_epoch_data.seed.clone();
    }
    if expected == *candidate {
        Ok(())
    } else {
        Err(IncorrectProtocolState(
            "received state differs from parent-derived state".to_owned(),
        ))
    }
}

/// Drop the SOK digest without reading a cached proof.
pub fn emitted_proof_statement(proof: &v2::LedgerProofProdStableV2) -> Statement {
    let stmt = &proof.statement;
    Statement {
        source: stmt.source.clone(),
        target: stmt.target.clone(),
        connecting_ledger_left: stmt.connecting_ledger_left.clone(),
        connecting_ledger_right: stmt.connecting_ledger_right.clone(),
        supply_increase: stmt.supply_increase.clone(),
        fee_excess: stmt.fee_excess.clone(),
        sok_digest: (),
    }
}
