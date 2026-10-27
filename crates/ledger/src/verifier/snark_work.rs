//! Verification of SNARK work offered to the snark pool.
//!
//! A transaction proof's statement carries a sok digest, the digest of the
//! fee and prover of the work it was made for, so that the proof can't be
//! re-advertised by someone else, for their own fee. A peer's work is
//! accepted only if every proof's sok digest is the digest of the work's
//! advertised fee and prover, and the proofs verify against their statements.
//!
//! Completed works in a staged ledger diff are checked the same way, by
//! [`super::Verifier::verify_transaction_snarks`].

use mina_core::snark::Snark;
use mina_curves::pasta::{Fq, Vesta};
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    v2::{LedgerProofProdStableV2, TransactionSnarkWorkTStableV2Proofs},
};
use poly_commitment::ipa::SRS;

use crate::{
    proofs::{verification::verify_transaction, VerifierIndex},
    scan_state::scan_state::transaction_snark::{SokDigest, SokMessage, Statement},
};

/// Why a batch of SNARK work was rejected. `work` is the index of the first
/// offending work in the batch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnarkWorkError {
    #[error("work {work}: invalid field element or public key")]
    InvalidInput { work: usize },
    #[error("work {work}: proof's sok digest does not match the work's fee and prover")]
    SokDigestMismatch { work: usize },
    #[error("proof verification failed")]
    VerificationFailed,
}

fn ledger_proofs(proofs: &TransactionSnarkWorkTStableV2Proofs) -> Vec<&LedgerProofProdStableV2> {
    match proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
    }
}

/// Checks that every proof of every work carries the sok digest of its work's
/// fee and prover, and returns the proofs' statements in order. This costs
/// one Blake2b hash per work, so it runs before any proof is verified.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/batcher.ml#L462-L475>
pub fn check_sok_digests(works: &[Snark]) -> Result<Vec<Statement<SokDigest>>, SnarkWorkError> {
    let mut statements = Vec::with_capacity(works.len() * 2);
    for (index, work) in works.iter().enumerate() {
        let invalid = |_: InvalidBigInt| SnarkWorkError::InvalidInput { work: index };
        let prover = (&work.snarker).try_into().map_err(invalid)?;
        let digest = SokMessage::create((&work.fee).into(), prover).digest();
        let (first, second) = work.proofs.statements();
        for statement in std::iter::once(first).chain(second) {
            let statement = Statement::<SokDigest>::try_from(statement).map_err(invalid)?;
            if statement.sok_digest != digest {
                return Err(SnarkWorkError::SokDigestMismatch { work: index });
            }
            statements.push(statement);
        }
    }
    Ok(statements)
}

/// Verifies a batch of SNARK work as the snark pool receives it: first the
/// sok digests ([`check_sok_digests`]), then all the proofs at once.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/batcher.ml#L467-L475>
pub fn verify_snark_works(
    works: &[Snark],
    verifier_index: &VerifierIndex<Fq>,
    srs: &SRS<Vesta>,
) -> Result<(), SnarkWorkError> {
    let statements = check_sok_digests(works)?;
    // Works being verified are decoded; a cached one is read back (PC1).
    let decoded = works
        .iter()
        .enumerate()
        .map(|(index, work)| {
            work.proofs
                .get()
                .map_err(|_| SnarkWorkError::InvalidInput { work: index })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let proofs = decoded
        .iter()
        .flat_map(|proofs| ledger_proofs(proofs))
        .map(|proof| {
            proof
                .0
                .proof
                .get()
                .map_err(|_| SnarkWorkError::VerificationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if verify_transaction(
        statements.iter().zip(proofs.iter().map(|p| &**p)),
        verifier_index,
        srs,
    ) {
        Ok(())
    } else {
        Err(SnarkWorkError::VerificationFailed)
    }
}

/// Real Mesa mainnet work, from precomputed blocks (`fixtures/README.md`).
/// These tests verify it against the embedded mainnet transaction key, so a
/// key or verifier regression fails them.
#[cfg(test)]
mod tests {
    use mina_curves::pasta::Fp;
    use mina_p2p_messages::{
        bigint::BigInt,
        binprot::BinProtRead,
        v2::{CurrencyFeeStableV1, TransactionSnarkWorkTStableV2},
    };
    use mina_signer::CompressedPubKey;
    use once_cell::sync::Lazy;

    use super::*;
    use crate::{
        proofs::verifiers::mainnet_transaction_verifier_index,
        scan_state::{currency::Fee, scan_state::transaction_snark::LedgerProof},
        verifier::get_srs,
    };

    /// `(name, binprot TransactionSnarkWorkTStableV2)`: two base proofs, one
    /// merge proof, and a merge and a base proof.
    const FIXTURES: &[(&str, &[u8])] = &[
        (
            "mainnet-551267-1",
            include_bytes!("fixtures/mainnet-551267-1.bin"),
        ),
        (
            "mainnet-556667-40",
            include_bytes!("fixtures/mainnet-556667-40.bin"),
        ),
        (
            "mainnet-548252-46",
            include_bytes!("fixtures/mainnet-548252-46.bin"),
        ),
    ];

    static INDEX: Lazy<VerifierIndex<Fq>> = Lazy::new(mainnet_transaction_verifier_index);

    fn fixture((name, bytes): (&str, &[u8])) -> TransactionSnarkWorkTStableV2 {
        TransactionSnarkWorkTStableV2::binprot_read(&mut &bytes[..])
            .unwrap_or_else(|e| panic!("fixture {name}: {e}"))
    }

    fn works() -> Vec<TransactionSnarkWorkTStableV2> {
        FIXTURES.iter().map(|&f| fixture(f)).collect()
    }

    fn verify(works: &[TransactionSnarkWorkTStableV2]) -> Result<(), SnarkWorkError> {
        let snarks: Vec<Snark> = works.iter().cloned().map(Snark::from).collect();
        verify_snark_works(&snarks, &INDEX, &get_srs::<Fp>())
    }

    fn first_proof(work: &mut TransactionSnarkWorkTStableV2) -> &mut LedgerProofProdStableV2 {
        match &mut work.proofs {
            TransactionSnarkWorkTStableV2Proofs::One(p) => p,
            TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => p,
        }
    }

    fn plus_one(x: &BigInt) -> BigInt {
        BigInt::from(x.to_field::<Fp>().unwrap() + Fp::from(1u64))
    }

    fn other_fee(work: &TransactionSnarkWorkTStableV2) -> CurrencyFeeStableV1 {
        let fee = Fee::from(&work.fee);
        (&Fee::from_u64(fee.as_u64() + 1)).into()
    }

    fn other_prover(work: &TransactionSnarkWorkTStableV2) -> CompressedPubKey {
        let mut prover: CompressedPubKey = (&work.prover).try_into().unwrap();
        prover.is_odd = !prover.is_odd;
        prover
    }

    #[test]
    fn pc2_cached_work_verifies_and_corruption_is_rejected() {
        use mina_p2p_messages::proof_cache::ProofStore;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-data/proof-cache/work-verifier-{}",
            std::process::id()
        ));
        let store = ProofStore::dir(&dir).unwrap();
        let works = works();
        assert_eq!(verify(&works), Ok(())); // warm the verifier and SRS
        let started = std::time::Instant::now();
        for _ in 0..3 {
            assert_eq!(verify(&works), Ok(()));
        }
        let decoded = started.elapsed() / 3;
        for work in &works {
            for p in ledger_proofs(&work.proofs) {
                p.0.proof.cache_in(&store).unwrap();
            }
        }
        let started = std::time::Instant::now();
        for _ in 0..3 {
            assert_eq!(verify(&works), Ok(()));
        }
        println!("PC2 work verification: 5 real proofs decoded {decoded:?}, cached {:?}, {} checked reads (warm, mean of 3)", started.elapsed() / 3, store.stats().reads);
        let file = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut bytes = std::fs::read(&file).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        std::fs::write(file, bytes).unwrap();
        assert!(matches!(
            verify(&works),
            Err(SnarkWorkError::InvalidInput { .. }) | Err(SnarkWorkError::VerificationFailed)
        ));
        drop(works);
        assert_eq!(store.stats().files, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The fixtures are what they claim to be: base (`N0`) and merge (`N2`)
    /// proofs, one work with two proofs, and each sok digest matching.
    #[test]
    fn fixtures_cover_base_and_merge_proofs() {
        let works = works();
        let kinds: Vec<String> = works
            .iter()
            .flat_map(|w| ledger_proofs(&w.proofs))
            .map(|p| {
                let branch =
                    &p.0.proof
                        .get()
                        .unwrap()
                        .0
                        .statement
                        .proof_state
                        .deferred_values
                        .branch_data;
                format!("{:?}", branch.proofs_verified)
            })
            .collect();
        assert!(kinds.iter().any(|k| k == "N0"), "{kinds:?}");
        assert!(kinds.iter().any(|k| k == "N2"), "{kinds:?}");
        assert!(works
            .iter()
            .any(|w| matches!(w.proofs, TransactionSnarkWorkTStableV2Proofs::Two(_))));
        let snarks: Vec<Snark> = works.into_iter().map(Snark::from).collect();
        assert_eq!(check_sok_digests(&snarks).unwrap().len(), kinds.len());
    }

    #[test]
    fn ocaml_completed_snarks_rpc_batch_verifies_and_rejects_a_bad_last_proof() {
        use mina_p2p_messages::{rpc::GetCompletedSnarksV1, rpc_kernel::PayloadBinprotReader};
        let bytes =
            include_bytes!("../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin");
        let mut works: Vec<_> = GetCompletedSnarksV1::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .0
            .into_iter()
            .collect();
        assert_eq!(verify(&works), Ok(()));
        let last = works.last_mut().unwrap();
        let proof = match &mut last.proofs {
            TransactionSnarkWorkTStableV2Proofs::One(p) => p,
            TransactionSnarkWorkTStableV2Proofs::Two((_, p)) => p,
        };
        let mut inner = (*proof.0.proof.get().unwrap()).clone();
        inner.0.proof.ft_eval1 = plus_one(&inner.0.proof.ft_eval1);
        proof.0.proof = inner.into();
        assert_eq!(verify(&works), Err(SnarkWorkError::VerificationFailed));
    }

    #[test]
    fn real_work_verifies_one_by_one_and_as_a_batch() {
        for &f in FIXTURES {
            assert_eq!(verify(&[fixture(f)]), Ok(()), "fixture {}", f.0);
        }
        assert_eq!(verify(&works()), Ok(()));
    }

    /// A bad challenge must fail the accumulator check regardless of which
    /// worker's partial sum contains it. The five proofs are real Mesa work.
    #[test]
    fn accumulator_batch_rejects_bad_challenge_across_worker_counts() {
        use crate::proofs::accumulator_check::accumulator_check;

        let works = works();
        let proofs: Vec<_> = works
            .iter()
            .flat_map(|work| ledger_proofs(&work.proofs))
            .map(|proof| proof.0.proof.get().unwrap().0.clone())
            .collect();
        assert_eq!(proofs.len(), 5);
        let srs = get_srs::<Fp>();

        for threads in [1, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                let valid: Vec<_> = proofs.iter().collect();
                assert!(
                    accumulator_check(&srs, &valid).unwrap(),
                    "{threads} threads"
                );

                for bad in [0, proofs.len() / 2, proofs.len() - 1] {
                    let mut corrupted = proofs.clone();
                    let limb = &mut corrupted[bad]
                        .statement
                        .proof_state
                        .deferred_values
                        .bulletproof_challenges
                        .0[0]
                        .prechallenge
                        .inner
                        .0[0]
                        .0;
                    limb.0 ^= 1;
                    let batch: Vec<_> = corrupted.iter().collect();
                    assert!(
                        !accumulator_check(&srs, &batch).unwrap(),
                        "{threads} threads, bad proof {bad}"
                    );
                }
            });
        }
    }

    /// A proof changed in one field fails, alone and in a batch of valid work.
    #[test]
    fn tampered_proof_fails() {
        for &f in FIXTURES {
            let mut work = fixture(f);
            let mut decoded = (*first_proof(&mut work).0.proof.get().unwrap()).clone();
            let proof = &mut decoded.0.proof;
            proof.ft_eval1 = plus_one(&proof.ft_eval1);
            first_proof(&mut work).0.proof = decoded.into();
            assert_eq!(
                verify(std::slice::from_ref(&work)),
                Err(SnarkWorkError::VerificationFailed),
                "fixture {}",
                f.0
            );
            let mut batch = works();
            batch.push(work);
            assert_eq!(verify(&batch), Err(SnarkWorkError::VerificationFailed));
        }
    }

    /// Work re-advertised with another fee is rejected by its sok digest,
    /// before any proof is verified.
    #[test]
    fn tampered_fee_fails() {
        let mut batch = works();
        let last = batch.len() - 1;
        batch[last].fee = other_fee(&batch[last]);
        let snarks: Vec<Snark> = batch.iter().cloned().map(Snark::from).collect();
        let mismatch = Err(SnarkWorkError::SokDigestMismatch { work: last });
        assert_eq!(check_sok_digests(&snarks).map(|_| ()), mismatch);
        assert_eq!(verify(&batch), mismatch);

        // The proofs themselves still verify against their statements, which
        // is all the snark pool checked before: only the sok digest check
        // catches this.
        let proofs: Vec<&LedgerProofProdStableV2> = batch
            .iter()
            .flat_map(|w| ledger_proofs(&w.proofs))
            .collect();
        let statements: Vec<Statement<SokDigest>> = proofs
            .iter()
            .map(|p| Statement::try_from(&p.0.statement).unwrap())
            .collect();
        let decoded = proofs
            .iter()
            .map(|p| p.0.proof.get().unwrap())
            .collect::<Vec<_>>();
        let pairs = statements.iter().zip(decoded.iter().map(|p| &**p));
        assert!(verify_transaction(pairs, &INDEX, &get_srs::<Fp>()));
    }

    /// Transaction proofs are verified in parallel batches of 32 (I5d4): a
    /// proof given another proof's statement is rejected wherever it falls,
    /// in the first batch, at a boundary, or in a partial tail.
    #[test]
    fn a_bad_proof_is_rejected_in_any_batch() {
        let works = works();
        let proofs: Vec<&LedgerProofProdStableV2> = works
            .iter()
            .flat_map(|w| ledger_proofs(&w.proofs))
            .collect();
        let statements: Vec<Statement<SokDigest>> = proofs
            .iter()
            .map(|p| Statement::try_from(&p.0.statement).unwrap())
            .collect();
        let decoded = proofs
            .iter()
            .map(|p| p.0.proof.get().unwrap())
            .collect::<Vec<_>>();
        let n = proofs.len();
        assert!(n >= 2);
        let srs = get_srs::<Fp>();
        for len in [1, 2, 31, 32, 33, 65] {
            let pairs = (0..len)
                .map(|i| (&statements[i % n], &*decoded[i % n]))
                .collect::<Vec<_>>();
            assert!(
                verify_transaction(pairs.iter().copied(), &INDEX, &srs),
                "valid batch {len}"
            );
            let mut positions = vec![0, 30, 31, 32, 33, 63, 64, len - 1];
            positions.retain(|&bad| bad < len);
            positions.sort_unstable();
            positions.dedup();
            for bad in positions {
                let mut pairs = pairs.clone();
                pairs[bad].0 = &statements[(bad + 1) % n];
                assert!(
                    !verify_transaction(pairs.iter().copied(), &INDEX, &srs),
                    "batch {len}, bad proof {bad}"
                );
            }
        }
    }

    /// Work re-advertised by another prover is rejected by its sok digest.
    #[test]
    fn tampered_prover_fails() {
        for (index, &f) in FIXTURES.iter().enumerate() {
            let mut batch = works();
            batch[index].prover = (&other_prover(&batch[index])).into();
            assert_eq!(
                verify(&batch),
                Err(SnarkWorkError::SokDigestMismatch { work: index }),
                "fixture {}",
                f.0
            );
        }
    }

    /// A thief who also rewrites the statements' sok digests to match their
    /// own fee and prover passes the digest check, but the proofs, which
    /// commit to the original digest, no longer verify.
    #[test]
    fn stolen_work_fails() {
        for &f in FIXTURES {
            let mut work = fixture(f);
            let fee = other_fee(&work);
            let prover = other_prover(&work);
            let digest = SokMessage::create((&fee).into(), prover.clone()).digest();
            work.fee = fee;
            work.prover = (&prover).into();
            match &mut work.proofs {
                TransactionSnarkWorkTStableV2Proofs::One(p) => {
                    p.0.statement.sok_digest = (&digest).into();
                }
                TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => {
                    p.0.statement.sok_digest = (&digest).into();
                    q.0.statement.sok_digest = (&digest).into();
                }
            }
            let snarks = [Snark::from(work.clone())];
            assert!(check_sok_digests(&snarks).is_ok(), "fixture {}", f.0);
            assert_eq!(
                verify(&[work]),
                Err(SnarkWorkError::VerificationFailed),
                "fixture {}",
                f.0
            );
        }
    }

    /// The staged ledger path, which checks the completed works of a block's
    /// diff, also rejects a proof whose sok digest isn't its work's.
    #[test]
    fn staged_ledger_verify_rejects_mismatched_sok_message() {
        let mut work = fixture(FIXTURES[0]);
        let fee = Fee::from(&other_fee(&work));
        let prover: CompressedPubKey = (&work.prover).try_into().unwrap();
        let proof = LedgerProof::try_from(&*first_proof(&mut work)).unwrap();
        let result = super::super::verify(vec![(proof, SokMessage::create(fee, prover))]);
        assert_eq!(
            result,
            Err("Transaction_snark.verify: Mismatched sok_message".to_string())
        );
    }
}
