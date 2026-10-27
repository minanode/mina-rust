use mina_p2p_messages::v2::{CurrencyFeeStableV1, NonZeroCurvePoint};
use std::sync::Arc;

use mina_snark::{TransactionVerifier, VerifierSRS};
use serde::{Deserialize, Serialize};

use crate::{
    external_snark_worker::{
        ExternalSnarkWorkerError, ExternalSnarkWorkerWorkError, IncludedWorkLog, SnarkWorkSpec,
    },
    SnarkerSetting,
};

#[derive(Serialize, Deserialize, Debug, Clone, derive_more::From)]
pub enum ExternalSnarkWorkerEvent {
    Started,
    Killed,
    /// The proof, with the key and fee it was made for.
    WorkResult(mina_core::snark::Snark),
    WorkError(ExternalSnarkWorkerWorkError),
    WorkCancelled,
    Error(ExternalSnarkWorkerError),
}

pub trait ExternalSnarkWorkerService {
    /// Starts the worker. Its proofs are checked with `work_verifier` and
    /// `verifier_srs` before they are offered.
    fn start(
        &mut self,
        public_key: NonZeroCurvePoint,
        fee: CurrencyFeeStableV1,
        work_verifier: TransactionVerifier,
        verifier_srs: Arc<VerifierSRS>,
    ) -> Result<(), ExternalSnarkWorkerError>;

    /// Submits snark work
    fn submit(&mut self, spec: SnarkWorkSpec) -> Result<(), ExternalSnarkWorkerError>;

    /// Cancel current work
    fn cancel(&mut self) -> Result<(), ExternalSnarkWorkerError>;

    /// Kills external process.
    fn kill(&mut self) -> Result<(), ExternalSnarkWorkerError>;

    /// Keeps the operator's worker setting for restarts, or forgets it
    /// (P5.4). Nothing by default.
    fn save_setting(&mut self, _setting: Option<&SnarkerSetting>) {}

    /// Keeps the log of our included work, if it changed since it was last
    /// kept (P5.4). Nothing by default.
    fn save_included_work(&mut self, _log: &IncludedWorkLog) {}
}
