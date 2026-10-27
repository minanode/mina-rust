//! Verify Mina blockchain SNARK proofs of blocks, natively or in the browser.
//!
//! A block whose proof verifies attests, by Pickles recursion, the validity of every
//! ancestor back to genesis, so a client can check the chain against an untrusted
//! data source. The verification key is supplied by the caller (the OCaml node's
//! `blockchainVerificationKey` JSON, or a mina-tree verifier-index JSON), so hard
//! forks such as Mesa need new data rather than new code.
//!
//! The verification-key ingester and the precomputed-block decoder are adapted from
//! [MinaProtocol/mina-verify](https://github.com/MinaProtocol/mina-verify)
//! (Apache-2.0).
//!
//! On `wasm32`, [`wasm`] exposes the same API to JavaScript.

mod gossip;
mod precomputed;
mod verifier_index;
#[cfg(target_family = "wasm")]
pub mod wasm;

use std::sync::Arc;

use ledger::{
    proofs::{verification::verify_block, VerifierIndex},
    verifier::get_srs,
};
use mina_core::consensus::consensus_take;
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::v2::{
    ConsensusProofOfStakeDataConsensusStateValueStableV2 as ConsensusState,
    MinaBlockHeaderStableV2, StateHash,
};
use serde::Serialize;

pub use gossip::header_from_gossip;
pub use precomputed::{header_from_precomputed, header_from_precomputed_value};
pub use verifier_index::{verifier_index_auto, verifier_index_from_json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The verification-key JSON could not be parsed into a verifier index.
    InvalidVerificationKey(String),
    /// The block could not be decoded.
    InvalidBlock(String),
    /// The block decoded, but its proof does not verify against the key.
    ProofInvalid,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidVerificationKey(e) => write!(f, "invalid verification key: {e}"),
            Error::InvalidBlock(e) => write!(f, "invalid block: {e}"),
            Error::ProofInvalid => write!(f, "block proof does not verify"),
        }
    }
}

impl std::error::Error for Error {}

/// Facts about a block that are attested by its verified proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockFacts {
    pub height: u32,
    pub global_slot_since_genesis: u32,
    pub state_hash: String,
    pub previous_state_hash: String,
    pub genesis_state_hash: String,
    pub staged_ledger_hash: String,
    pub staking_epoch_ledger_hash: String,
    /// Minimum chain density over the sliding window. A chain built by a minority
    /// of stake is sparse, so this is low when fed a private fork.
    pub min_window_density: u32,
    /// Block timestamp, milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
}

/// A block whose proof verified, with what fork choice needs to compare it.
#[derive(Debug, Clone)]
pub struct VerifiedBlock {
    pub facts: BlockFacts,
    pub state_hash: StateHash,
    pub consensus_state: ConsensusState,
}

impl VerifiedBlock {
    fn from_header(header: &MinaBlockHeaderStableV2) -> Result<Self, Error> {
        let state_hash = header
            .try_hash()
            .map_err(|e| Error::InvalidBlock(format!("hashing protocol state: {e:?}")))?;
        Ok(Self {
            facts: BlockFacts::from_header(header, &state_hash),
            state_hash,
            consensus_state: header.protocol_state.body.consensus_state.clone(),
        })
    }
}

impl BlockFacts {
    fn from_header(header: &MinaBlockHeaderStableV2, state_hash: &StateHash) -> Self {
        let state = &header.protocol_state;
        let consensus = &state.body.consensus_state;
        let blockchain = &state.body.blockchain_state;
        Self {
            height: consensus.blockchain_length.as_u32(),
            global_slot_since_genesis: consensus.global_slot_since_genesis.as_u32(),
            state_hash: state_hash.to_string(),
            previous_state_hash: state.previous_state_hash.to_string(),
            genesis_state_hash: state.body.genesis_state_hash.to_string(),
            staged_ledger_hash: blockchain
                .staged_ledger_hash
                .non_snark
                .ledger_hash
                .to_string(),
            staking_epoch_ledger_hash: consensus.staking_epoch_data.ledger.hash.to_string(),
            min_window_density: consensus.min_window_density.as_u32(),
            timestamp_ms: blockchain.timestamp.0 .0.as_u64(),
        }
    }
}

/// Picks the best tip among verified blocks with the node's own fork-choice rule
/// (Ouroboros Samasika: chain length for short-range forks, window density for
/// long-range ones), using only the blocks' proven consensus states.
#[derive(Debug, Default)]
pub struct ForkChoice {
    best: Option<VerifiedBlock>,
}

impl ForkChoice {
    /// Offer a verified block. Returns whether it became the best tip.
    pub fn consider(&mut self, block: &VerifiedBlock) -> bool {
        let take = match &self.best {
            None => true,
            Some(best) => {
                best.state_hash != block.state_hash
                    && consensus_take(
                        &best.consensus_state,
                        &block.consensus_state,
                        &best.state_hash,
                        &block.state_hash,
                    )
            }
        };
        if take {
            self.best = Some(block.clone());
        }
        take
    }

    pub fn best(&self) -> Option<&VerifiedBlock> {
        self.best.as_ref()
    }
}

#[cfg(not(target_family = "wasm"))]
fn default_work_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("mina-web-verifier")
}

#[cfg(target_family = "wasm")]
fn default_work_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("/mina-web-verifier")
}

/// A blockchain-SNARK verifier for one network's verification key.
#[derive(Clone)]
pub struct Verifier {
    index: Arc<VerifierIndex<Fq>>,
}

impl Verifier {
    /// Build a verifier from a verification-key JSON in either supported format.
    ///
    /// This computes the SRS and Lagrange basis for the key's domain, which is the
    /// expensive part of setup; reuse the verifier for many blocks.
    pub fn from_vk_json(json: &str) -> Result<Self, Error> {
        // mina-tree dumps failed verifications into the work dir, and panics if
        // none is set. Without a filesystem (wasm) the dump just fails and is logged.
        mina_core::init_work_dir_if_unset(default_work_dir);
        let index =
            verifier_index::verifier_index_auto(json).map_err(Error::InvalidVerificationKey)?;
        Ok(Self {
            index: Arc::new(index),
        })
    }

    /// Build the global SRS used when checking proofs, so the first verification
    /// does not pay for it.
    pub fn warm_up(&self) {
        let _ = get_srs::<Fp>();
    }

    /// Verify a block header's blockchain-SNARK proof.
    ///
    /// Only `protocol_state` and `protocol_state_proof` take part in verification.
    pub fn verify_header(&self, header: &MinaBlockHeaderStableV2) -> bool {
        let srs = get_srs::<Fp>();
        verify_block(header, &self.index, &srs)
    }

    fn verify_and_extract(&self, header: &MinaBlockHeaderStableV2) -> Result<VerifiedBlock, Error> {
        if !self.verify_header(header) {
            return Err(Error::ProofInvalid);
        }
        VerifiedBlock::from_header(header)
    }

    /// Decode a precomputed block (the JSON form nodes publish) and verify its proof.
    pub fn verify_precomputed(&self, json: &str) -> Result<VerifiedBlock, Error> {
        self.verify_and_extract(&header_from_precomputed(json)?)
    }

    /// Decode a block from a raw consensus gossip payload and verify its proof.
    pub fn verify_gossip(&self, payload: &[u8]) -> Result<VerifiedBlock, Error> {
        self.verify_and_extract(&header_from_gossip(payload)?)
    }
}
