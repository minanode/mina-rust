use malloc_size_of::MallocSizeOf;
use mina_p2p_messages::{
    binprot::macros::{BinProtRead, BinProtWrite},
    v2::{
        CurrencyFeeStableV1, MinaBaseFeeWithProverStableV1,
        MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        MinaStateSnarkedLedgerStateStableV2, MinaStateSnarkedLedgerStateWithSokStableV2,
        NetworkPoolSnarkPoolDiffVersionedStableV2,
        NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1, NonZeroCurvePoint,
        TransactionSnarkWorkStatementStableV2, TransactionSnarkWorkTStableV2,
    },
};
use serde::{Deserialize, Serialize};

use super::{
    snark_proofs::{ProofUnavailable, SnarkProofs},
    SnarkInfo, SnarkJobId,
};

#[derive(BinProtRead, BinProtWrite, Serialize, Deserialize, Debug, Clone)]
pub struct Snark {
    pub snarker: NonZeroCurvePoint,
    pub fee: CurrencyFeeStableV1,
    /// Decoded, or cached once the snark pool stores the work (PC1).
    pub proofs: SnarkProofs,
}

impl Snark {
    pub fn job_id(&self) -> SnarkJobId {
        self.proofs.job_id()
    }

    pub fn info(&self) -> SnarkInfo {
        SnarkInfo {
            job_id: self.job_id(),
            fee: self.fee.clone(),
            prover: self.snarker.clone(),
        }
    }

    /// This work with its proofs in the node's proof cache (PC1).
    pub fn cached(&self) -> Self {
        Self {
            snarker: self.snarker.clone(),
            fee: self.fee.clone(),
            proofs: self.proofs.cache(),
        }
    }

    /// The snark pool diff that offers this work, as OCaml builds it:
    /// `Add_solved_work (statement, {proof; fee = {fee; prover}})`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L27-L29>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/priced_proof.ml#L8-L10>
    pub fn gossip_diff(
        &self,
    ) -> Result<NetworkPoolSnarkPoolDiffVersionedStableV2, ProofUnavailable> {
        Ok(NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(
            Box::new((self.statement(), self.try_into()?)),
        ))
    }

    pub fn statement(&self) -> TransactionSnarkWorkStatementStableV2 {
        // TODO(binier): move conversion to mina-p2p-messages-rs
        fn conv_stmt(
            stmt: &MinaStateSnarkedLedgerStateWithSokStableV2,
        ) -> MinaStateSnarkedLedgerStateStableV2 {
            let v = MinaStateBlockchainStateValueStableV2LedgerProofStatement {
                source: stmt.source.clone(),
                target: stmt.target.clone(),
                connecting_ledger_left: stmt.connecting_ledger_left.clone(),
                connecting_ledger_right: stmt.connecting_ledger_right.clone(),
                supply_increase: stmt.supply_increase.clone(),
                fee_excess: stmt.fee_excess.clone(),
                sok_digest: (),
            };
            MinaStateSnarkedLedgerStateStableV2(v)
        }
        match self.proofs.statements() {
            (first, None) => TransactionSnarkWorkStatementStableV2::One(conv_stmt(first)),
            (first, Some(second)) => {
                TransactionSnarkWorkStatementStableV2::Two((conv_stmt(first), conv_stmt(second)))
            }
        }
    }

    pub fn tie_breaker_hash(&self) -> [u8; 32] {
        super::tie_breaker_hash(&self.job_id(), &self.snarker)
    }

    /// The work as the wire carries it, its proofs decoded (from the cache
    /// if they are cached).
    pub fn to_wire(&self) -> Result<TransactionSnarkWorkTStableV2, ProofUnavailable> {
        Ok(TransactionSnarkWorkTStableV2 {
            fee: self.fee.clone(),
            proofs: self.proofs.get()?.as_ref().clone(),
            prover: self.snarker.clone(),
        })
    }
}

impl From<TransactionSnarkWorkTStableV2> for Snark {
    fn from(value: TransactionSnarkWorkTStableV2) -> Self {
        Self {
            snarker: value.prover,
            fee: value.fee,
            proofs: value.proofs.into(),
        }
    }
}

impl From<NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1> for Snark {
    fn from(value: NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1) -> Self {
        Self {
            snarker: value.fee.prover,
            fee: value.fee.fee,
            proofs: value.proof.into(),
        }
    }
}

impl TryFrom<&Snark> for NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1 {
    type Error = ProofUnavailable;

    fn try_from(value: &Snark) -> Result<Self, Self::Error> {
        Ok(Self {
            proof: value.proofs.get()?.as_ref().clone(),
            fee: MinaBaseFeeWithProverStableV1 {
                fee: value.fee.clone(),
                prover: value.snarker.clone(),
            },
        })
    }
}

impl MallocSizeOf for Snark {
    fn size_of(&self, ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        self.proofs.size_of(ops)
    }
}
