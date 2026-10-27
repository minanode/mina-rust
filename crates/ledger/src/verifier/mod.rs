use self::common::CheckResult;
use crate::{
    proofs::{
        self, field::FieldWitness, verification, verifiers::TransactionVerifier, VerifierIndex,
    },
    scan_state::{
        scan_state::transaction_snark::{LedgerProof, SokMessage, TransactionSnark},
        transaction_logic::{valid, verifiable, zkapp_statement::ZkappStatement, WithStatus},
    },
    staged_ledger::staged_ledger::SkipVerification,
    VerificationKey,
};
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::v2::{
    PicklesProofProofsVerified2ReprStableV2, PicklesProofProofsVerifiedMaxStableV2,
};
use mina_signer::CompressedPubKey;
use once_cell::sync::Lazy;
use poly_commitment::{ipa::SRS, SRS as _};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Verifier;

// TODO: Move this into `Verifier` struct above
pub static VERIFIER_INDEX: Lazy<Arc<VerifierIndex<Fq>>> = Lazy::new(|| {
    TransactionVerifier::get()
        .expect("verifier index not initialized")
        .into()
});

/// Returns the Structured Reference String (SRS) for proof verification.
/// Lazily created and cached for the whole process, shared by every thread:
/// it is immutable, and creating one takes about 2 s, which the per-thread
/// cache this replaced (`cache!`) made the ledger thread pay at its first
/// verified block, after the node's own had been made (I5d4).
///
/// TODO: Use directly from proof-systems (<https://github.com/o1-labs/mina-rust/issues/1749>)
pub fn get_srs<F: FieldWitness>() -> Arc<SRS<F::OtherCurve>> {
    use std::any::{Any, TypeId};

    type Cached = Vec<(TypeId, Box<dyn Any + Send + Sync>)>;
    // Held while an SRS is created, so that it is created once.
    static CACHE: Mutex<Cached> = Mutex::new(Vec::new());

    let type_id = TypeId::of::<F>();
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, srs)) = cache.iter().find(|(id, _)| *id == type_id) {
        return srs
            .downcast_ref::<Arc<SRS<F::OtherCurve>>>()
            .expect("cached by its field's type id")
            .clone();
    }
    let srs = Arc::new(SRS::<F::OtherCurve>::create(
        <F as proofs::field::FieldWitness>::Scalar::SRS_DEPTH,
    ));
    cache.push((type_id, Box::new(srs.clone())));
    srs
}

/// Returns the SRS on the other curve (Mutex-wrapped version for prover).
///
/// TODO: Use directly from proof-systems (<https://github.com/o1-labs/mina-rust/issues/1749>)
pub fn get_srs_mut<F: FieldWitness>() -> Arc<Mutex<SRS<F::OtherCurve>>> {
    cache! {
        Arc<Mutex<SRS<F::OtherCurve>>>,
        {
            let srs = SRS::<F::OtherCurve>::create(<F as proofs::field::FieldWitness>::Scalar::SRS_DEPTH);
            Arc::new(Mutex::new(srs))
        }
    }
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/transaction_snark/transaction_snark.ml#L3492>
fn verify(ts: Vec<(LedgerProof, SokMessage)>) -> Result<(), String> {
    let srs = get_srs::<Fp>();

    if ts.iter().all(|(proof, msg)| {
        let LedgerProof(TransactionSnark { statement, .. }, _) = proof;
        statement.sok_digest == msg.digest()
    }) {
        let verifier_index = VERIFIER_INDEX.as_ref();

        let decoded = ts
            .iter()
            .map(|(p, _)| p.0.proof.get().map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let proofs = ts
            .iter()
            .zip(&decoded)
            .map(|((p, _), proof)| (&p.0.statement, &**proof));

        if !crate::proofs::verification::verify_transaction(proofs, verifier_index, &srs) {
            return Err("Transaction_snark.verify: verification failed".into());
        }
        Ok(())
    } else {
        Err("Transaction_snark.verify: Mismatched sok_message".into())
    }
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/verifier/dummy.ml#L59C1-L75C81>
#[cfg(test)]
fn verify_digest_only(ts: Vec<(LedgerProof, SokMessage)>) -> Result<(), String> {
    use crate::scan_state::scan_state::transaction_snark::SokDigest;

    if ts.iter().all(|(proof, msg)| {
        let LedgerProof(TransactionSnark { statement, .. }, _) = proof;
        statement.sok_digest == SokDigest::default() || statement.sok_digest == msg.digest()
    }) {
        Ok(())
    } else {
        Err("Transaction_snark.verify: Mismatched sok_message".into())
    }
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/verifier/verifier_intf.ml#L10C1-L36C29>
pub type VerifyCommandsResult = Result<valid::UserCommand, VerifierError>;

#[derive(Debug, thiserror::Error)]
pub enum VerifierError {
    // TODO(adonagy): print something here as well?
    #[error("Batch verification failed")]
    ValidAssuming(
        Vec<(
            VerificationKey,
            ZkappStatement,
            Arc<PicklesProofProofsVerifiedMaxStableV2>,
        )>,
    ),
    #[error("Invalid keys: {0:?}")]
    InvalidKeys(Vec<CompressedPubKey>),
    #[error("Invalid signature: {0:?}")]
    InvalidSignature(Vec<CompressedPubKey>),
    #[error("Invalid proof: {0}")]
    InvalidProof(String),
    #[error("Missing verification key: {0:?}")]
    MissingVerificationKey(Vec<CompressedPubKey>),
    #[error("Unexpected verification key: {0:?}")]
    UnexpectedVerificationKey(Vec<CompressedPubKey>),
    #[error("Mismatched verification key: {0:?}")]
    MismatchedVerificationKey(Vec<CompressedPubKey>),
    #[error("Authorization kind does not match the authorization - Keys {0:?}")]
    MismatchedAuthorizationKind(Vec<CompressedPubKey>),
}

impl Verifier {
    pub fn verify(&self, _proofs: &[Arc<LedgerProof>]) -> Result<Result<(), ()>, String> {
        // Implement verification later
        //
        // <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/pickles/pickles.ml#L1122>
        // <https://viable-systems.slack.com/archives/D01SVA87PQC/p1671715846448749>
        Ok(Ok(()))
    }

    /// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/verifier/prod.ml#L138>
    #[allow(unreachable_code)]
    pub fn verify_transaction_snarks(
        &self,
        ts: Vec<(LedgerProof, SokMessage)>,
    ) -> Result<(), String> {
        #[cfg(test)]
        return verify_digest_only(ts);

        verify(ts)
    }

    pub fn verify_commands(
        &self,
        cmds: Vec<WithStatus<verifiable::UserCommand>>,
        skip_verification: Option<SkipVerification>,
    ) -> Vec<VerifyCommandsResult> {
        // Each command's keys and signatures, independent of the others: in
        // parallel, in order (I5d4).
        let cs: Vec<_> = {
            use rayon::prelude::*;
            cmds.into_par_iter().map(common::check).collect()
        };

        let mut to_verify = cs
            .iter()
            .filter_map(|c| match c {
                CheckResult::Valid(_) => None,
                CheckResult::ValidAssuming((_, xs)) => Some(xs),
                _ => None,
            })
            .flatten();

        let all_verified = if skip_verification.is_some() {
            true
        } else {
            let srs = get_srs::<Fp>();

            to_verify.all(|(vk, zkapp_statement, proof)| {
                let proof: PicklesProofProofsVerified2ReprStableV2 = (&**proof).into();
                verification::verify_zkapp(vk, zkapp_statement, &proof, &srs)
            })
        };

        cs.into_iter()
            .map(|c| match c {
                CheckResult::Valid(c) => Ok(c),
                CheckResult::ValidAssuming((c, xs)) => {
                    if all_verified {
                        // Verified pool commands can outlive the scan state.
                        if let valid::UserCommand::ZkAppCommand(command) = &c {
                            command.zkapp_command.account_updates.fold((), |(), update| {
                                if let crate::scan_state::transaction_logic::zkapp_command::Control::Proof(proof) = &update.authorization {
                                    if let Err(error) = proof.cache() {
                                        mina_core::log::inner::warn!("proof cache: {error}");
                                    }
                                }
                            });
                        }
                        Ok(c)
                    } else {
                        Err(VerifierError::ValidAssuming(xs))
                    }
                }
                CheckResult::InvalidKeys(keys) => Err(VerifierError::InvalidKeys(keys)),
                CheckResult::InvalidSignature(keys) => Err(VerifierError::InvalidSignature(keys)),
                CheckResult::InvalidProof(s) => Err(VerifierError::InvalidProof(s)),
                CheckResult::MissingVerificationKey(keys) => {
                    Err(VerifierError::MissingVerificationKey(keys))
                }
                CheckResult::UnexpectedVerificationKey(keys) => {
                    Err(VerifierError::UnexpectedVerificationKey(keys))
                }
                CheckResult::MismatchedAuthorizationKind(keys) => {
                    Err(VerifierError::MismatchedAuthorizationKind(keys))
                }
            })
            .collect()
    }
}

pub mod common;
pub mod snark_work;

#[cfg(test)]
mod srs_tests {
    use super::*;

    /// One SRS per field for the whole process (I5d4): a thread that
    /// verifies after another one has made it doesn't make its own.
    #[test]
    fn the_srs_is_made_once_and_shared_by_all_threads() {
        let here = get_srs::<Fp>();
        let there = std::thread::spawn(get_srs::<Fp>).join().unwrap();
        assert!(Arc::ptr_eq(&here, &there));
        assert!(Arc::ptr_eq(&here, &get_srs::<Fp>()));
        let other = std::thread::spawn(get_srs::<Fq>).join().unwrap();
        assert!(Arc::ptr_eq(&other, &get_srs::<Fq>()));
        assert_ne!(here.g.len(), 0);
        assert_ne!(other.g.len(), 0);
    }
}
