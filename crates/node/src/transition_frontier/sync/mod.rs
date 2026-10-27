pub mod ledger;

mod transition_frontier_sync_state;
pub use transition_frontier_sync_state::*;

mod transition_frontier_sync_actions;
pub use transition_frontier_sync_actions::*;

mod transition_frontier_sync_reducer;

mod transition_frontier_sync_effects;

mod transition_frontier_sync_reported;
pub use transition_frontier_sync_reported::*;

#[cfg(test)]
mod transition_frontier_sync_tests;

use mina_core::block::{prevalidate::BlockPrevalidationError, ArcBlockWithHash};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerBlockFetchError {
    Timeout,
    Disconnected,
    DataUnavailable,
    /// The block has the hash asked for, but not a delta block chain proof or
    /// a body that go with its header.
    Invalid(BlockPrevalidationError),
    /// The block's body could not be checked here: a proof it holds is
    /// cached and could not be read back (BP-SAFE). Not the peer's fault.
    LocalProofUnavailable(String),
}

impl PeerBlockFetchError {
    /// What a fetched block that failed its header and body checks counts
    /// as. A local proof read failure says nothing about the peer.
    pub fn from_block_check(error: BlockPrevalidationError) -> Self {
        match error {
            BlockPrevalidationError::BodyProofUnavailable { error } => {
                Self::LocalProofUnavailable(error)
            }
            error => Self::Invalid(error),
        }
    }
}

#[derive(thiserror::Error, Serialize, Deserialize, Debug, Clone)]
pub enum SyncError {
    #[error("sync failed due to block({}, {}) application error: {1}", .0.height(), .0.hash())]
    BlockApplyFailed(ArcBlockWithHash, String),
    /// A block of a frontier store restore's replay failed to apply (I5d1):
    /// the store is at fault, not the block, which is not blacklisted.
    #[error("restore failed due to block({}, {}) application error: {1}", .0.height(), .0.hash())]
    RestoredBlockApplyFailed(ArcBlockWithHash, String),
}

impl SyncError {
    pub fn failed_block(&self) -> &ArcBlockWithHash {
        match self {
            Self::BlockApplyFailed(block, _) | Self::RestoredBlockApplyFailed(block, _) => block,
        }
    }
}

/// How close to the best tip we have to be for the full
/// verification of proofs contained in the block
/// body (zkApp txns and completed works) to be enabled.
pub(crate) const CATCHUP_BLOCK_VERIFY_TAIL_LENGTH: usize = 5;
