use std::sync::Arc;

use mina_node_account::AccountSecretKey;
use mina_p2p_messages::v2::{
    ConsensusBodyReferenceStableV1, LedgerProofProdStableV2, MinaBasePendingCoinbaseUpdateStableV1,
    MinaBasePendingCoinbaseWitnessStableV2, MinaBaseSparseLedgerBaseStableV2,
    MinaBaseStagedLedgerHashStableV1, ProverExtendBlockchainInputStableV2,
    StagedLedgerDiffDiffStableV2, StateHash,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StagedLedgerDiffCreateOutput {
    pub diff: StagedLedgerDiffDiffStableV2,
    /// `protocol_state.blockchain_state.body_reference`
    pub diff_hash: ConsensusBodyReferenceStableV1,
    pub staged_ledger_hash: MinaBaseStagedLedgerHashStableV1,
    pub emitted_ledger_proof: Option<Arc<LedgerProofProdStableV2>>,
    pub pending_coinbase_update: MinaBasePendingCoinbaseUpdateStableV1,
    pub pending_coinbase_witness: MinaBasePendingCoinbaseWitnessStableV2,
    pub stake_proof_sparse_ledger: MinaBaseSparseLedgerBaseStableV2,
}

/// The node asks for proofs by block hash and gets them back as a
/// `BlockProducerEvent::BlockProve` event. How and in which memory they are
/// made is the service's business: the web node proves in an isolated
/// instance and never holds a `BlockProver` (I34c).
pub trait BlockProducerService {
    fn prove(
        &mut self,
        block_hash: StateHash,
        input: Box<ProverExtendBlockchainInputStableV2>,
        deadline: redux::Timestamp,
    );
    fn with_producer_keypair<T>(&self, f: impl FnOnce(&AccountSecretKey) -> T) -> Option<T>;
    /// Keeps the block production history across restarts, if it changed
    /// since it was last kept (F-producer). Called on each status change, in
    /// block production's path: it must not block (hand the write to
    /// another thread).
    fn save_block_production(&mut self) {}
}
