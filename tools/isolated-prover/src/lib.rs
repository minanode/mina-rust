//! I34/I34b: a byte boundary around the existing Mesa payment and block provers.
//! The browser experiments use public fixtures, including the genesis winner.

use ledger::{
    proofs::{verification::verify_transaction, verifiers::TransactionVerifier},
    scan_state::{
        currency::Fee,
        scan_state::transaction_snark::{SokMessage, Statement},
    },
};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{self, BinProtRead},
    v2,
};
use mina_signer::CompressedPubKey;

#[cfg(target_family = "wasm")]
mod wasm;

pub type Input = (
    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    v2::TransactionWitnessStableV2,
);

/// Prototype transport budget, not a measured maximum for production witnesses.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub fn decode<T: BinProtRead>(bytes: &[u8]) -> anyhow::Result<T> {
    anyhow::ensure!(
        bytes.len() <= MAX_MESSAGE_BYTES,
        "message exceeds 1 MiB limit"
    );
    let mut remaining = bytes;
    let value = T::binprot_read(&mut remaining)?;
    anyhow::ensure!(remaining.is_empty(), "trailing bytes in binprot message");
    Ok(value)
}

/// Verify the returned block proof against the original request's next state
/// and the mainnet key. P4.5's changed-genesis-hash and previous-state controls
/// must both fail; no public input supplied by the prover is trusted.
pub fn verify_block(
    input: &[u8],
    proof: &[u8],
    verifier: &ledger::proofs::verifiers::BlockVerifier,
) -> anyhow::Result<()> {
    let input = decode::<v2::ProverExtendBlockchainInputStableV2>(input)?;
    let proof = decode::<v2::MinaBaseProofStableV2>(proof)?;
    let srs = ledger::verifier::get_srs::<Fp>();
    let verifies = |state: &v2::MinaStateProtocolStateValueStableV2| {
        let header = v2::MinaBlockHeaderStableV2 {
            protocol_state: state.clone(),
            protocol_state_proof: std::sync::Arc::new(proof.clone()),
            delta_block_chain_proof: (
                state.previous_state_hash.clone(),
                std::iter::empty().collect(),
            ),
            current_protocol_version: mina_core::constants::PROTOCOL_VERSION,
            proposed_protocol_version_opt: None,
        };
        ledger::proofs::verification::verify_block(&header, verifier, &srs)
    };
    anyhow::ensure!(
        verifies(&input.next_state),
        "block proof failed mainnet verification"
    );
    anyhow::ensure!(
        !verifies(&input.chain.state),
        "previous state unexpectedly verified"
    );
    // P4.5's control: another genesis hash, the previous state's hash. In the
    // first block after genesis that is the genesis hash itself, so take the
    // hash before it (I34c's first produced block).
    let mut tampered = input.next_state.clone();
    tampered.body.genesis_state_hash =
        if tampered.previous_state_hash != tampered.body.genesis_state_hash {
            tampered.previous_state_hash.clone()
        } else {
            input.chain.state.previous_state_hash.clone()
        };
    anyhow::ensure!(
        tampered.body.genesis_state_hash != input.next_state.body.genesis_state_hash,
        "no distinct genesis hash for the tampered-input control"
    );
    anyhow::ensure!(!verifies(&tampered), "tampered input unexpectedly verified");
    Ok(())
}

/// The web node's bridge messages (I34c), as
/// `mina_node_common::block_producer::isolated` writes them at bridge
/// version 1. Mirrored here so the verifier stays independent of the node.
#[derive(binprot::macros::BinProtRead)]
pub struct BridgeRequest {
    pub version: u32,
    pub request_id: u64,
    pub block_hash: v2::StateHash,
    pub deadline_ms: u64,
    pub is_genesis: bool,
    pub input: v2::ProverExtendBlockchainInputStableV2,
}

#[derive(binprot::macros::BinProtRead)]
pub struct BridgeResponse {
    pub version: u32,
    pub request_id: u64,
    pub block_hash: v2::StateHash,
    pub proof: v2::MinaBaseProofStableV2,
}

/// Checks one proof the web node's isolated prover returned, from the bytes
/// the bridge carried: the response answers the request, the request holds
/// no producer key and names its own next state's hash, and the proof
/// verifies for that state with the mainnet key (and not for the previous
/// state or a tampered one, see [`verify_block`]).
pub fn verify_bridge(
    request: &[u8],
    response: &[u8],
    verifier: &ledger::proofs::verifiers::BlockVerifier,
) -> anyhow::Result<BridgeRequest> {
    use mina_p2p_messages::binprot::BinProtWrite;
    let request = decode::<BridgeRequest>(request)?;
    let response = decode::<BridgeResponse>(response)?;
    anyhow::ensure!(
        request.version == 1 && response.version == 1,
        "bridge version"
    );
    anyhow::ensure!(
        response.request_id == request.request_id && response.block_hash == request.block_hash,
        "the response answers another request"
    );
    anyhow::ensure!(
        request.input.prover_state.producer_private_key
            == v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one()),
        "the request carries a producer key"
    );
    anyhow::ensure!(
        request.input.next_state.try_hash()? == request.block_hash,
        "the request's block hash is not its next state's"
    );
    let mut input = Vec::new();
    request.input.binprot_write(&mut input)?;
    let mut proof = Vec::new();
    response.proof.binprot_write(&mut proof)?;
    verify_block(&input, &proof, verifier)?;
    Ok(request)
}

pub fn message(fee: u64) -> SokMessage {
    // The public prover identity used by mesa_payment_proof_verifies.
    SokMessage {
        fee: Fee::from_u64(fee),
        prover: CompressedPubKey::from_address(
            "B62qpK6TcG4sWdtT3BzdbWHiK3RJMj3Zbo9mqwBos7cVsydPMCj5wZx",
        )
        .unwrap(),
    }
}

/// Verify against the mainnet transaction key, with a changed-SOK negative
/// control. The statement is read from the original input, never the prover.
pub fn verify(input: &[u8], proof: &[u8], verifier: &TransactionVerifier) -> anyhow::Result<()> {
    let (statement, _) = decode::<Input>(input)?;
    let statement = Statement::<()>::try_from(&statement)?;
    let proof = decode::<v2::TransactionSnarkProofStableV2>(proof)?;
    let srs = ledger::verifier::get_srs::<Fp>();
    anyhow::ensure!(
        verify_transaction(
            [(&statement.clone().with_digest(message(0).digest()), &proof)],
            verifier,
            &srs,
        ),
        "proof failed mainnet verification"
    );
    anyhow::ensure!(
        !verify_transaction(
            [(&statement.with_digest(message(1).digest()), &proof)],
            verifier,
            &srs,
        ),
        "changed SOK digest unexpectedly verified"
    );
    Ok(())
}
