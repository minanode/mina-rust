use ledger::Account;
use mina_core::snark::Snark;
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use crate::p2p::PeerId;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkSource {
    Gossip,
    CompletedSnarks,
    Rpc,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionStage {
    Received(WorkSource),
    Verified,
    Revalidate,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProverAccount {
    Missing,
    Present { can_receive: bool },
}

impl ProverAccount {
    pub fn from_account(account: Option<&Account>) -> Self {
        account.map_or(Self::Missing, |account| Self::Present {
            // None_given satisfies only Auth_required.None. This also fails
            // closed for the legacy Both variant rather than invoking its
            // unimplemented general permission evaluator.
            can_receive: matches!(account.permissions.access, ledger::AuthRequired::None)
                && matches!(account.permissions.receive, ledger::AuthRequired::None),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    InsufficientFee,
    ProverCannotReceive,
    WorkNotReferenced,
    NoFrontier,
    AccountUnavailable,
    NotCheaper,
    Busy,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L370-L424>
pub fn check_prover(
    fee: u64,
    account_creation_fee: u64,
    account: ProverAccount,
) -> Result<(), AdmissionError> {
    match account {
        ProverAccount::Missing if fee < account_creation_fee => {
            Err(AdmissionError::InsufficientFee)
        }
        ProverAccount::Present { can_receive: false } => Err(AdmissionError::ProverCannotReceive),
        _ => Ok(()),
    }
}

/// The ledger worker looks up only the default-token account in this exact
/// best-tip ledger. The node rechecks the tip and full statement on return.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProverAccountRequest {
    pub id: u64,
    pub best_tip: v2::StateHash,
    pub ledger_hash: v2::LedgerHash,
    pub work: Snark,
    pub sender: PeerId,
    pub is_sender_local: bool,
    pub stage: AdmissionStage,
}

impl PartialEq for ProverAccountRequest {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AdmissionStats {
    pub received_gossip: u64,
    pub received_completed_snarks: u64,
    pub received_rpc: u64,
    pub candidates: u64,
    pub admitted: u64,
    pub failed_verification_batches: u64,
    pub works_in_failed_verification_batches: u64,
    /// Work bundles handed to the libp2p forwarding action, not peer deliveries.
    pub forwarded: u64,
    pub insufficient_fee: u64,
    pub prover_cannot_receive: u64,
    pub work_not_referenced: u64,
    pub no_frontier: u64,
    pub account_unavailable: u64,
    pub not_cheaper: u64,
    pub busy: u64,
    pub rejected_after_verification: u64,
    pub revalidation_removed: u64,
    pub stale_account_results: u64,
}

impl AdmissionStats {
    pub fn received(&mut self, source: WorkSource) {
        let counter = match source {
            WorkSource::Gossip => &mut self.received_gossip,
            WorkSource::CompletedSnarks => &mut self.received_completed_snarks,
            WorkSource::Rpc => &mut self.received_rpc,
        };
        *counter = counter.saturating_add(1);
    }

    pub fn rejected(&mut self, stage: AdmissionStage, error: AdmissionError) {
        let counter = match stage {
            AdmissionStage::Verified => &mut self.rejected_after_verification,
            AdmissionStage::Revalidate => &mut self.revalidation_removed,
            AdmissionStage::Received(_) => match error {
                AdmissionError::InsufficientFee => &mut self.insufficient_fee,
                AdmissionError::ProverCannotReceive => &mut self.prover_cannot_receive,
                AdmissionError::WorkNotReferenced => &mut self.work_not_referenced,
                AdmissionError::NoFrontier => &mut self.no_frontier,
                AdmissionError::AccountUnavailable => &mut self.account_unavailable,
                AdmissionError::NotCheaper => &mut self.not_cheaper,
                AdmissionError::Busy => &mut self.busy,
            },
        };
        *counter = counter.saturating_add(1);
    }
}
