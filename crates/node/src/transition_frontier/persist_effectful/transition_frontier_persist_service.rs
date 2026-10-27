use mina_core::{block::ArcBlockWithHash, ChainId};
use mina_p2p_messages::v2::StateHash;

use crate::persist::{
    load::{BlockProofVerifier, PersistLoadRequest},
    ChainEntry,
};

/// The node's side of the store thread (R20 D3). Checkpoints don't go
/// through it: the ledger thread hands them to the store thread itself.
pub trait TransitionFrontierPersistService: redux::Service {
    /// Opens the chain's store on the store thread. `false` if there is no
    /// store thread.
    fn persist_open(&mut self, chain_id: ChainId, genesis_state_hash: StateHash) -> bool;

    /// Hands a best chain, root first, to the store thread, with its blocks
    /// (it writes only those not stored yet). `false` if there is no store
    /// thread.
    fn persist_chain_put(&mut self, chain: Vec<ChainEntry>, blocks: Vec<ArcBlockWithHash>) -> bool;

    /// Asks the store thread to load the store for a restore (I5d1): it
    /// checks the stored blocks' proofs with `verifier`, hands what it loads
    /// to the ledger thread, then reports. `false` if there is no store
    /// thread.
    fn persist_load(
        &mut self,
        request: PersistLoadRequest,
        verifier: Option<BlockProofVerifier>,
    ) -> bool;

    /// Request cancellation. False if ledger handoff already won the race.
    /// Bootstrap must wait for the worker acknowledgement when true.
    fn persist_load_cancel(&mut self) -> bool;

    /// Number of completed, checked load stages.
    fn persist_load_progress(&self) -> u64 {
        0
    }

    /// The restore is committed: the store thread clears the restore
    /// attempts record.
    fn persist_restore_done(&mut self) -> bool;

    /// A restore failed: the store thread deletes the store.
    fn persist_discard(&mut self, reason: String) -> bool;
}
