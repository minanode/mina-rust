mod snark_work_verify_effectful_actions;
pub use snark_work_verify_effectful_actions::*;

mod snark_work_verify_effects;

mod snark_work_verify_service;
pub use snark_work_verify_service::*;

use serde::{Deserialize, Serialize};

pub struct SnarkWorkVerifyIdType;
impl mina_core::requests::RequestIdType for SnarkWorkVerifyIdType {
    fn request_id_type() -> &'static str {
        "SnarkWorkVerifyId"
    }
}

pub type SnarkWorkVerifyId = mina_core::requests::RequestId<SnarkWorkVerifyIdType>;

#[derive(Serialize, Deserialize, Debug, Clone, thiserror::Error)]
pub enum SnarkWorkVerifyError {
    #[error("verification failed")]
    VerificationFailed,
    #[error("validator thread crashed")]
    ValidatorThreadCrashed,
    /// A proof's sok digest isn't the digest of the work's fee and prover:
    /// the work is someone else's, re-advertised.
    #[error("sok digest does not match the work's fee and prover")]
    SokDigestMismatch,
}
