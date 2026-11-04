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
use poly_commitment::{commitment::CommitmentCurve, ipa::SRS, SRS as _};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

#[derive(Debug, Clone)]
pub struct Verifier;

// TODO: Move this into `Verifier` struct above
pub static VERIFIER_INDEX: Lazy<Arc<VerifierIndex<Fq>>> = Lazy::new(|| {
    TransactionVerifier::get()
        .expect("verifier index not initialized")
        .into()
});

type SrsCache = Vec<(std::any::TypeId, Box<dyn std::any::Any + Send + Sync>)>;

/// The SRS of each field, made once for the whole process.
static SRS_CACHE: Mutex<SrsCache> = Mutex::new(Vec::new());

fn srs_cache() -> std::sync::MutexGuard<'static, SrsCache> {
    SRS_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn cached_srs<F: FieldWitness>(cache: &SrsCache) -> Option<Arc<SRS<F::OtherCurve>>> {
    let type_id = std::any::TypeId::of::<F>();
    cache.iter().find(|(id, _)| *id == type_id).map(|(_, srs)| {
        srs.downcast_ref::<Arc<SRS<F::OtherCurve>>>()
            .expect("cached by its field's type id")
            .clone()
    })
}

/// Returns the Structured Reference String (SRS) for proof verification.
/// Lazily created and cached for the whole process, shared by every thread:
/// it is immutable, and creating one takes about 2 s, which the per-thread
/// cache this replaced (`cache!`) made the ledger thread pay at its first
/// verified block, after the node's own had been made (I5d4).
///
/// TODO: Use directly from proof-systems (<https://github.com/o1-labs/mina-rust/issues/1749>)
pub fn get_srs<F: FieldWitness>() -> Arc<SRS<F::OtherCurve>> {
    // Held while an SRS is created, so that it is created once.
    let mut cache = srs_cache();
    if let Some(srs) = cached_srs::<F>(&cache) {
        return srs;
    }
    let srs = Arc::new(SRS::<F::OtherCurve>::create(
        <F as proofs::field::FieldWitness>::Scalar::SRS_DEPTH,
    ));
    cache.push((std::any::TypeId::of::<F>(), Box::new(srs.clone())));
    srs
}

/// [`get_srs`], made on the current rayon pool if it isn't made yet:
/// `SRS::create_parallel` gives the same points, in the same order, as
/// `SRS::create`, in a fraction of the 2.4 s a web node's start spent on it.
/// Only for a process whose rayon pool is built (the web node, after
/// `init_rayon`): elsewhere rayon would build a default pool first.
///
/// The cache's lock is not held while the SRS is made: a thread that held it
/// while it waited for the pool could block every pool thread that asks for
/// the SRS meanwhile, and itself if it is one of them and takes such a job.
/// A [`get_srs`] at the same moment may then make one too; every caller gets
/// the first one stored.
pub fn get_srs_parallel<F: FieldWitness>() -> Arc<SRS<F::OtherCurve>>
where
    <F::OtherCurve as CommitmentCurve>::Map: Sync,
{
    if let Some(srs) = cached_srs::<F>(&srs_cache()) {
        return srs;
    }
    let srs = Arc::new(SRS::<F::OtherCurve>::create_parallel(
        <F as proofs::field::FieldWitness>::Scalar::SRS_DEPTH,
    ));
    let mut cache = srs_cache();
    if let Some(first) = cached_srs::<F>(&cache) {
        return first;
    }
    cache.push((std::any::TypeId::of::<F>(), Box::new(srs.clone())));
    srs
}

/// Returns the SRS on the other curve (Mutex-wrapped version for prover).
/// Each thread has its own, made at its first proof.
///
/// A panic in a proof that holds the lock poisons it, and the `unwrap` of
/// every later proof on that thread then panicked too: one failed devnet
/// job made the SNARK worker fail every job after it until a restart. A
/// poisoned SRS is not reused, even though no proof changes it but for its
/// cache of Lagrange bases, which a panic leaves without the basis being
/// added: this thread's SRS is made again, as for its first proof, so a
/// panic costs one job. [`prover_srs_rebuilds`] counts the rebuilds.
///
/// TODO: Use directly from proof-systems (<https://github.com/o1-labs/mina-rust/issues/1749>)
pub fn get_srs_mut<F: FieldWitness>() -> Arc<Mutex<SRS<F::OtherCurve>>> {
    use std::{
        any::{Any, TypeId},
        cell::RefCell,
        mem::ManuallyDrop,
    };

    // As `cache!`, but an entry can be replaced. One static for every `F`,
    // keyed by its type id.
    thread_local! {
        static CACHE: ManuallyDrop<RefCell<Vec<(TypeId, Box<dyn Any>)>>> =
            const { ManuallyDrop::new(RefCell::new(Vec::new())) };
    }

    let type_id = TypeId::of::<F>();
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(index) = cache.iter().position(|(id, _)| *id == type_id) {
            let srs = cache[index]
                .1
                .downcast_ref::<Arc<Mutex<SRS<F::OtherCurve>>>>()
                .expect("cached by its field's type id");
            if !srs.is_poisoned() {
                return srs.clone();
            }
            // Dropped before its replacement is made: never two at once.
            cache.swap_remove(index);
            let rebuilds = PROVER_SRS_REBUILDS.fetch_add(1, Ordering::Relaxed) + 1;
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "ProverSrsRebuilt",
                summary = "a panic poisoned this thread's prover SRS: it is made again",
                thread = std::thread::current().name().unwrap_or("<unnamed>").to_owned(),
                rebuilds = rebuilds,
            );
        }
        let srs = Arc::new(Mutex::new(SRS::<F::OtherCurve>::create(
            <F as proofs::field::FieldWitness>::Scalar::SRS_DEPTH,
        )));
        cache.push((type_id, Box::new(srs.clone())));
        srs
    })
}

static PROVER_SRS_REBUILDS: AtomicU64 = AtomicU64::new(0);

/// How many times a prover SRS was made again after a panic poisoned it
/// ([`get_srs_mut`]), in this process.
pub fn prover_srs_rebuilds() -> u64 {
    PROVER_SRS_REBUILDS.load(Ordering::Relaxed)
}

/// Test-only hooks for proving tests, here and in the node crates (the
/// `test-hooks` feature, which only their dev-dependencies enable).
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_hooks {
    use std::sync::atomic::{AtomicBool, Ordering};

    static PANIC_HOLDING_SRS: AtomicBool = AtomicBool::new(false);

    /// The next step proof, on any thread, panics while it holds its
    /// thread's prover SRS lock ([`super::get_srs_mut`]), as the devnet
    /// zkApp panic did.
    pub fn panic_next_proof_holding_srs() {
        PANIC_HOLDING_SRS.store(true, Ordering::SeqCst);
    }

    pub(crate) fn srs_locked() {
        if PANIC_HOLDING_SRS.swap(false, Ordering::SeqCst) {
            panic!("test hook: the proof panicked holding the prover SRS lock");
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

    /// The SRS made on the pool is the one `SRS::create` makes: its points
    /// in the same order, and the same blinder.
    #[test]
    fn the_srs_made_in_parallel_is_the_sequential_one() {
        const DEPTH: usize = 256;
        let fp = SRS::<<Fp as FieldWitness>::OtherCurve>::create(DEPTH);
        let par = SRS::<<Fp as FieldWitness>::OtherCurve>::create_parallel(DEPTH);
        assert_eq!(par.g, fp.g);
        assert_eq!(par.h, fp.h);
        let fq = SRS::<<Fq as FieldWitness>::OtherCurve>::create(DEPTH);
        let par = SRS::<<Fq as FieldWitness>::OtherCurve>::create_parallel(DEPTH);
        assert_eq!(par.g, fq.g);
        assert_eq!(par.h, fq.h);
    }

    /// `get_srs_parallel` and `get_srs` share one SRS per field: whichever
    /// is first makes it, and the other returns it.
    #[test]
    fn the_parallel_and_the_sequential_srs_are_one() {
        let parallel = get_srs_parallel::<Fp>();
        assert!(Arc::ptr_eq(&parallel, &get_srs::<Fp>()));
        assert_eq!(parallel.g.len(), <Fp as FieldWitness>::Scalar::SRS_DEPTH);
        let sequential = get_srs::<Fq>();
        assert!(Arc::ptr_eq(&sequential, &get_srs_parallel::<Fq>()));
    }

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

    /// A panic holding a thread's prover SRS lock costs that one use: the
    /// next one on the thread gets a new SRS, equal to the first, with its
    /// own Lagrange bases, and the other field's SRS is kept.
    #[test]
    fn a_poisoned_prover_srs_is_made_again() {
        use ark_poly::EvaluationDomain as _;

        std::thread::spawn(|| {
            let first = get_srs_mut::<Fq>();
            let other = get_srs_mut::<Fp>();
            assert!(Arc::ptr_eq(&first, &get_srs_mut::<Fq>()));
            let domain = ark_poly::Radix2EvaluationDomain::new(1 << 4).unwrap();
            let basis = first.lock().unwrap().get_lagrange_basis(domain).clone();
            let rebuilds = prover_srs_rebuilds();

            let panicked = std::panic::catch_unwind(|| {
                let srs = get_srs_mut::<Fq>();
                let _locked = srs.lock().unwrap();
                panic!("a failed proof");
            });
            assert!(panicked.is_err());
            assert!(first.is_poisoned());

            let second = get_srs_mut::<Fq>();
            assert!(!Arc::ptr_eq(&first, &second));
            assert!(!second.is_poisoned());
            assert_eq!(prover_srs_rebuilds(), rebuilds + 1);
            {
                let (first, second) = (
                    first.lock().unwrap_or_else(|p| p.into_inner()),
                    second.lock().unwrap(),
                );
                assert_eq!(first.g, second.g);
                assert_eq!(first.h, second.h);
                assert!(!second.lagrange_bases().contains_key(&domain.size()));
                assert_eq!(*second.get_lagrange_basis(domain), basis);
            }
            assert!(Arc::ptr_eq(&second, &get_srs_mut::<Fq>()));
            assert!(Arc::ptr_eq(&other, &get_srs_mut::<Fp>()));
        })
        .join()
        .unwrap();
    }
}
