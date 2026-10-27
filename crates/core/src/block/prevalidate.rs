use super::{ArcBlockWithHash, BlockHeader};
use crate::constants::PROTOCOL_VERSION;
use mina_p2p_messages::v2::StateHash;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BlockPrevalidationError {
    GenesisNotReady,
    BlockAfterStopSlot,
    NonEmptyStagedLedgerDiffAfterStopSlot,
    ReceivedTooEarly {
        current_global_slot: u32,
        block_global_slot: u32,
    },
    ReceivedTooLate {
        current_global_slot: u32,
        block_global_slot: u32,
        delta: u32,
    },
    InvalidGenesisProtocolState,
    InvalidProtocolVersion,
    MismatchedProtocolVersion,
    ConsantsMismatch,
    InvalidDeltaBlockChainProof,
    /// The body's hash is not the header's `body_reference`.
    InvalidBodyReference {
        expected: String,
        got: String,
    },
    /// The body's hash could not be computed here: a proof it holds is
    /// cached and could not be read back. A local failure, not the sender's.
    BodyProofUnavailable {
        error: String,
    },
}

impl BlockPrevalidationError {
    /// Whether a block with this error's hash can be rejected for good.
    ///
    /// The state hash covers neither the delta block chain proof nor the body,
    /// so a peer can pair an honest header with a bad proof or body: the block
    /// may still arrive intact from another peer.
    pub fn is_forever_invalid(&self) -> bool {
        !matches!(
            self,
            Self::ReceivedTooEarly { .. }
                | Self::InvalidDeltaBlockChainProof
                | Self::InvalidBodyReference { .. }
                | Self::BodyProofUnavailable { .. }
        )
    }
}

pub fn validate_block_timing(
    block: &ArcBlockWithHash,
    genesis: &ArcBlockWithHash,
    cur_global_slot: u32,
    allow_block_too_late: bool,
) -> Result<(), BlockPrevalidationError> {
    let block_global_slot = block.global_slot();
    let delta = genesis.constants().delta.as_u32();

    if cur_global_slot < block_global_slot {
        return Err(BlockPrevalidationError::ReceivedTooEarly {
            current_global_slot: cur_global_slot,
            block_global_slot,
        });
    } else if !allow_block_too_late && cur_global_slot.saturating_sub(block_global_slot) > delta {
        return Err(BlockPrevalidationError::ReceivedTooLate {
            current_global_slot: cur_global_slot,
            block_global_slot,
            delta,
        });
    }

    Ok(())
}

pub fn validate_genesis_state(
    block: &ArcBlockWithHash,
    genesis: &ArcBlockWithHash,
) -> Result<(), BlockPrevalidationError> {
    if block.header().genesis_state_hash() != genesis.hash() {
        return Err(BlockPrevalidationError::InvalidGenesisProtocolState);
    }
    Ok(())
}

pub fn validate_protocol_versions(block: &ArcBlockWithHash) -> Result<(), BlockPrevalidationError> {
    let min_transaction_version = 1.into();
    let v = &block.header().current_protocol_version;
    let nv = block
        .header()
        .proposed_protocol_version_opt
        .as_ref()
        .unwrap_or(v);

    // Our version values are unsigned, so there is no need to check that the
    // other parts are not negative.
    let valid =
        v.transaction >= min_transaction_version && nv.transaction >= min_transaction_version;
    if !valid {
        return Err(BlockPrevalidationError::InvalidProtocolVersion);
    }

    let compatible =
        v.transaction == PROTOCOL_VERSION.transaction && v.network == PROTOCOL_VERSION.network;
    if !compatible {
        return Err(BlockPrevalidationError::MismatchedProtocolVersion);
    }

    Ok(())
}

pub fn validate_constants(
    block: &ArcBlockWithHash,
    genesis: &ArcBlockWithHash,
) -> Result<(), BlockPrevalidationError> {
    // NOTE: currently these cannot change between blocks, but that
    // may not always be true?
    if block.constants() != genesis.constants() {
        return Err(BlockPrevalidationError::ConsantsMismatch);
    }
    Ok(())
}

/// The delta block chain proof `(init, body_hashes)` must lead to the block's
/// parent: starting from `init`, each state body hash in turn is hashed with
/// the state hash so far into the next state hash, and the last one must be
/// `previous_state_hash`. As in OCaml, the list's length is not checked: with
/// `delta = 0`, as on mainnet and devnet, the list is empty and `init` is the
/// parent's hash.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/mina_block/validation.ml#L378-L389>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/transition_chain_verifier/transition_chain_verifier.ml#L5-L18>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/merkle_list_verifier/merkle_list_verifier.ml#L36-L50>
pub fn validate_delta_block_chain(header: &BlockHeader) -> Result<(), BlockPrevalidationError> {
    let (init, body_hashes) = &header.delta_block_chain_proof;
    let last = body_hashes
        .iter()
        .try_fold(init.clone(), |state_hash, body_hash| {
            StateHash::try_from_hashes(&state_hash, body_hash)
        })
        .map_err(|_| BlockPrevalidationError::InvalidDeltaBlockChainProof)?;
    if last != header.protocol_state.previous_state_hash {
        return Err(BlockPrevalidationError::InvalidDeltaBlockChainProof);
    }
    Ok(())
}

/// The checks of a received block that need neither its parent nor its body.
/// The node checks the body's hash against the header too
/// (`State::prevalidate_block`), and verifies the proof after these pass.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/transition_router/initial_validator.ml#L280-L287>
/// checks the time received, the genesis state, the proof, the delta block
/// chain proof and the protocol versions.
pub fn prevalidate_block(
    block: &ArcBlockWithHash,
    genesis: &ArcBlockWithHash,
    cur_global_slot: u32,
    allow_block_too_late: bool,
) -> Result<(), BlockPrevalidationError> {
    validate_stop_slots(block, crate::constants::stop_slots())?;
    validate_block_timing(block, genesis, cur_global_slot, allow_block_too_late)?;
    validate_genesis_state(block, genesis)?;
    validate_delta_block_chain(block.header())?;
    validate_protocol_versions(block)?;
    validate_constants(block, genesis)?;
    Ok(())
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_handler/validator.ml#L42-L106>
pub fn validate_stop_slots(
    block: &ArcBlockWithHash,
    stops: crate::constants::StopSlots,
) -> Result<(), BlockPrevalidationError> {
    validate_stop_slots_diff(block.global_slot(), block.staged_ledger_diff(), stops)
}

pub fn validate_stop_slots_diff(
    slot: u32,
    diff: &mina_p2p_messages::v2::StagedLedgerDiffDiffDiffStableV2,
    stops: crate::constants::StopSlots,
) -> Result<(), BlockPrevalidationError> {
    if !stops.blocks_allowed(slot) {
        return Err(BlockPrevalidationError::BlockAfterStopSlot);
    }
    if !stops.transactions_allowed(slot) && !staged_ledger_diff_is_empty(diff) {
        return Err(BlockPrevalidationError::NonEmptyStagedLedgerDiffAfterStopSlot);
    }
    Ok(())
}

/// Empty means no second pre-diff, work, commands, coinbase or statuses.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/staged_ledger_diff/diff.ml#L512-L524>
pub fn staged_ledger_diff_is_empty(
    diff: &mina_p2p_messages::v2::StagedLedgerDiffDiffDiffStableV2,
) -> bool {
    use mina_p2p_messages::v2::StagedLedgerDiffDiffPreDiffWithAtMostTwoCoinbaseStableV2Coinbase as Coinbase;
    let (first, second) = (&diff.0, &diff.1);
    second.is_none()
        && first.completed_works.is_empty()
        && first.commands.is_empty()
        && matches!(first.coinbase, Coinbase::Zero)
        && first.internal_command_statuses.is_empty()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mina_p2p_messages::{
        binprot::BinProtRead,
        list::List,
        v2::{MinaBlockBlockStableV2, StateBodyHash, StateHash},
    };

    use super::{prevalidate_block, validate_delta_block_chain, BlockPrevalidationError};
    use crate::block::BlockWithHash;

    /// A mainnet block at 563978 from live gossip: an 8-byte length, the
    /// `NewState` tag, then the block.
    const GOSSIP: &[u8] =
        include_bytes!("../../../web-verifier/tests/fixtures/mainnet-563978.gossipbin");

    fn block() -> MinaBlockBlockStableV2 {
        MinaBlockBlockStableV2::binprot_read(&mut &GOSSIP[9..]).unwrap()
    }

    fn body_hash(block: &MinaBlockBlockStableV2) -> StateBodyHash {
        block.header.protocol_state.body.try_hash().unwrap().into()
    }

    fn invalid(block: &MinaBlockBlockStableV2) -> bool {
        matches!(
            validate_delta_block_chain(&block.header),
            Err(BlockPrevalidationError::InvalidDeltaBlockChainProof)
        )
    }

    #[test]
    fn live_block_has_a_valid_proof() {
        let block = block();
        // Mainnet's delta is 0: the proof is the parent's hash alone.
        let (init, body_hashes) = &block.header.delta_block_chain_proof;
        assert!(body_hashes.is_empty());
        assert_eq!(init, &block.header.protocol_state.previous_state_hash);
        validate_delta_block_chain(&block.header).unwrap();
    }

    #[test]
    fn proof_must_lead_to_the_parent() {
        let mut block = block();
        let genesis = block.header.protocol_state.body.genesis_state_hash.clone();
        block.header.delta_block_chain_proof.0 = genesis;
        assert!(invalid(&block));
    }

    /// With delta > 0 the proof starts `delta` blocks back and hashes in the
    /// body of each block up to the parent, in order.
    #[test]
    fn longer_proofs() {
        let mut block = block();
        let start = block.header.protocol_state.body.genesis_state_hash.clone();
        let first = body_hash(&block);
        let mut other = block.clone();
        other
            .header
            .protocol_state
            .body
            .blockchain_state
            .timestamp
            .0
             .0 = (other
            .header
            .protocol_state
            .body
            .blockchain_state
            .timestamp
            .0
             .0
            .as_u64()
            + 1)
        .into();
        let second = body_hash(&other);
        let middle = StateHash::try_from_hashes(&start, &first).unwrap();
        let parent = StateHash::try_from_hashes(&middle, &second).unwrap();

        block.header.protocol_state.previous_state_hash = parent;
        block.header.delta_block_chain_proof = (
            start.clone(),
            [first.clone(), second.clone()].into_iter().collect(),
        );
        validate_delta_block_chain(&block.header).unwrap();

        // Out of order, short of the parent, or past it.
        let proofs: [List<StateBodyHash>; 3] = [
            [second.clone(), first.clone()].into_iter().collect(),
            [first.clone()].into_iter().collect(),
            [first.clone(), second.clone(), first].into_iter().collect(),
        ];
        for body_hashes in proofs {
            block.header.delta_block_chain_proof = (start.clone(), body_hashes);
            assert!(invalid(&block));
        }

        // The hash before the last body is a proof of its own.
        block.header.delta_block_chain_proof = (middle, [second].into_iter().collect());
        validate_delta_block_chain(&block.header).unwrap();
    }

    #[test]
    fn prevalidation_checks_the_proof() {
        let mut block = block();
        let genesis = BlockWithHash {
            hash: block.header.protocol_state.body.genesis_state_hash.clone(),
            block: Arc::new(block.clone()),
        };
        let cur_global_slot = genesis.global_slot();
        let prevalidate = |block: &MinaBlockBlockStableV2| {
            let block = BlockWithHash::try_new(Arc::new(block.clone())).unwrap();
            prevalidate_block(&block, &genesis, cur_global_slot, false)
        };
        prevalidate(&block).unwrap();

        block.header.delta_block_chain_proof.0 = genesis.hash.clone();
        let error = prevalidate(&block).unwrap_err();
        assert!(matches!(
            error,
            BlockPrevalidationError::InvalidDeltaBlockChainProof
        ));
        assert!(!error.is_forever_invalid());
    }
}
