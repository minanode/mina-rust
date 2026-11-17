//! The proofs this process has verified, so that it verifies each one once.
//!
//! A transaction proof is verified when it arrives as SNARK work for the
//! snark pool, and again in every block that buys it. A zkApp proof is
//! verified when its command enters the transaction pool, and again in the
//! block that includes the command. The second check repeats the first one:
//! whether a proof verifies depends only on the verification key, the
//! statement and the proof, and on the SRS, of which a process has one.
//!
//! So the verifier keeps the SHA-256 of those three for each proof it
//! accepted ([`VerifiedProof`]), and skips a proof whose digest it holds.
//! The statement is digested as the verifier reads it, as field elements. A
//! hit means this process verified these exact inputs before. Anything else
//! misses and is verified as before: another proof for the same statement,
//! the same proof for another statement, the same proof for another fee or
//! prover (the statement holds their sok digest), and the same proof under
//! another verification key. Checks that don't verify a proof are not
//! skipped: signatures, a work's sok digest against its fee and prover, a
//! work's statement against its scan state job, and the verification key a
//! zkApp account holds in the block's ledger.
//!
//! Only accepted proofs are kept, in memory, for the life of the process,
//! the [`CAPACITY`] most recent. Losing an entry costs one verification. A
//! batch with a proof that has no digest, because it can't be encoded, is
//! given to the verifier whole, which refuses it as before.
//!
//! OCaml skips work it finds in its snark pool with the same statement, fee,
//! prover and proofs, and commands it finds in its transaction pool:
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/staged_ledger/staged_ledger.ml#L690-L734>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/staged_ledger/check_commands.ml#L17-L34>

use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
};

use ark_ff::PrimeField;
use mina_curves::pasta::{Fp, Fq, Vesta};
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{
        MinaBaseVerificationKeyWireStableV1, PicklesProofProofsVerified2ReprStableV2,
        TransactionSnarkProofStableV2,
    },
};
use once_cell::sync::Lazy;
use poly_commitment::ipa::SRS;
use sha2::{Digest, Sha256};

use crate::{
    proofs::{
        to_field_elements::ToFieldElements,
        verification::{verify_transaction, verify_zkapps, ZkappProofToVerify},
        verifiers::TransactionVerifier,
        VerifierIndex,
    },
    scan_state::{
        scan_state::transaction_snark::{SokDigest, Statement},
        transaction_logic::zkapp_statement::ZkappStatement,
    },
    VerificationKey,
};

/// Verified proofs kept, the most recent: enough for a transaction pool
/// full of proof-heavy zkApp commands and the SNARK work of the blocks that
/// include them. A full set takes about 6 MiB when it first fills, and about
/// 10 MiB once its hash table has grown under replacement (estimates from
/// the collections' sizes, not measured).
pub const CAPACITY: usize = 1 << 16;

/// What identifies a verified proof: the SHA-256 of its verification key,
/// statement and proof.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VerifiedProof([u8; 32]);

/// Digests the parts of a key, each with its length, after a tag that keeps
/// the kinds of proof apart.
fn digest(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update((tag.len() as u64).to_le_bytes());
    sha.update(tag);
    for part in parts {
        sha.update((part.len() as u64).to_le_bytes());
        sha.update(part);
    }
    sha.finalize().into()
}

/// `None` for a value its encoder refuses, an array longer than its type
/// allows for one.
fn binprot(value: &impl BinProtWrite, size_hint: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(size_hint);
    value.binprot_write(&mut bytes).ok()?;
    Some(bytes)
}

/// A statement as the verifier reads it: its field elements, the proof's
/// public input. Statements with the same field elements verify alike.
fn statement_bytes(statement: &impl ToFieldElements<Fp>) -> Vec<u8> {
    let fields = statement.to_field_elements_owned();
    let mut bytes = Vec::with_capacity(fields.len() * 32);
    for field in fields {
        for limb in field.into_bigint().0 {
            bytes.extend_from_slice(&limb.to_le_bytes());
        }
    }
    bytes
}

impl VerifiedProof {
    /// A transaction proof under this process's transaction verification
    /// key. The statement includes the sok digest of the work's fee and
    /// prover. `None` if the proof can't be encoded.
    pub fn transaction(
        statement: &Statement<SokDigest>,
        proof: &TransactionSnarkProofStableV2,
    ) -> Option<Self> {
        Some(Self(digest(
            b"transaction proof",
            &[&statement_bytes(statement), &binprot(proof, 16 * 1024)?],
        )))
    }

    /// A zkApp proof under the verification key with this digest
    /// ([`verification_key_digest`]). `None` if the proof can't be encoded.
    pub fn zkapp(
        verification_key: &[u8; 32],
        statement: &ZkappStatement,
        proof: &PicklesProofProofsVerified2ReprStableV2,
    ) -> Option<Self> {
        Some(Self(digest(
            b"zkApp proof",
            &[
                verification_key,
                &statement_bytes(statement),
                &binprot(proof, 16 * 1024)?,
            ],
        )))
    }
}

/// The digest of everything in a zkApp verification key that its proofs are
/// verified with. `None` if the key can't be encoded.
pub fn verification_key_digest(verification_key: &VerificationKey) -> Option<[u8; 32]> {
    let wire = MinaBaseVerificationKeyWireStableV1::from(verification_key);
    Some(digest(
        b"zkApp verification key",
        &[&binprot(&wire, 4 * 1024)?],
    ))
}

/// The keys of a batch, or `None` if one of its proofs has none, or if
/// making them panics: the proofs come from peers, and the verifier, which
/// then gets the whole batch, contains its own panics.
fn keys_of_batch(keys: impl FnOnce() -> Option<Vec<VerifiedProof>>) -> Option<Vec<VerifiedProof>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(keys))
        .ok()
        .flatten()
}

struct Held {
    proofs: HashSet<VerifiedProof>,
    /// The same proofs, oldest first.
    order: VecDeque<VerifiedProof>,
}

impl Held {
    fn add(&mut self, proof: VerifiedProof) {
        if self.proofs.contains(&proof) {
            return;
        }
        while self.order.len() >= CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.proofs.remove(&oldest);
            }
        }
        self.proofs.insert(proof);
        self.order.push_back(proof);
    }
}

/// A set of verified proofs. The node has one, used by
/// [`verify_transaction_once`] and [`verify_zkapps_once`].
pub struct VerifiedProofs {
    held: Mutex<Held>,
    skipped: AtomicU64,
    verified: AtomicU64,
}

/// Counts of a [`VerifiedProofs`], since it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Proofs not verified because they had been.
    pub skipped: u64,
    /// Proofs verified and accepted.
    pub verified: u64,
    /// Verified proofs held now, at most [`CAPACITY`].
    pub held: usize,
}

impl Default for VerifiedProofs {
    fn default() -> Self {
        Self::new()
    }
}

impl VerifiedProofs {
    pub fn new() -> Self {
        Self {
            held: Mutex::new(Held {
                proofs: HashSet::new(),
                order: VecDeque::new(),
            }),
            skipped: AtomicU64::new(0),
            verified: AtomicU64::new(0),
        }
    }

    /// A panic while the lock is held leaves at worst a proof in one
    /// collection and not the other: a proof that is never dropped, or never
    /// found. Neither makes an unverified proof pass.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn stats(&self) -> Stats {
        Stats {
            skipped: self.skipped.load(Ordering::Relaxed),
            verified: self.verified.load(Ordering::Relaxed),
            held: self.held().order.len(),
        }
    }

    /// Verifies the proofs not verified before, all of them with one call of
    /// `verify`, and remembers them if it accepts them. `true` if every proof
    /// is verified, now or before.
    ///
    /// Each item is a proof with its key, which must be made from everything
    /// `verify` reads of that proof. `verify` gets the unverified items, in
    /// order, and returns whether all of them verify. Nothing is remembered
    /// when it refuses or panics. The lock is not held while it runs: two
    /// threads given the same proof at once may both verify it.
    pub fn verify_once<T>(
        &self,
        items: Vec<(VerifiedProof, T)>,
        verify: impl FnOnce(&[T]) -> bool,
    ) -> bool {
        let total = items.len();
        let (keys, unverified): (Vec<VerifiedProof>, Vec<T>) = {
            let held = self.held();
            items
                .into_iter()
                .filter(|(key, _)| !held.proofs.contains(key))
                .unzip()
        };
        self.skipped
            .fetch_add((total - unverified.len()) as u64, Ordering::Relaxed);
        if unverified.is_empty() {
            return true;
        }
        if !verify(&unverified) {
            return false;
        }
        self.verified
            .fetch_add(keys.len() as u64, Ordering::Relaxed);
        let mut held = self.held();
        for key in keys {
            held.add(key);
        }
        true
    }
}

static NODE: Lazy<VerifiedProofs> = Lazy::new(VerifiedProofs::new);

static ENABLED: Lazy<AtomicBool> =
    Lazy::new(|| AtomicBool::new(std::env::var_os("MINA_VERIFY_EVERY_PROOF").is_none()));

/// Whether verified proofs are skipped. They are unless the process started
/// with `MINA_VERIFY_EVERY_PROOF` set, which makes it verify every proof each
/// time, as before: for measuring verification, and for ruling this out.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns skipping verified proofs on or off, for benchmarks and tests that
/// measure or exercise verification itself.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// The counts of the node's verified proofs.
pub fn stats() -> Stats {
    NODE.stats()
}

fn is_the_node_srs(srs: &SRS<Vesta>) -> bool {
    std::ptr::eq(&*super::get_srs::<Fp>(), srs)
}

/// Whether `verifier_index` and `srs` are this process's transaction
/// verifier index and SRS, the ones every transaction proof of the node is
/// verified with. A proof verified with another index or SRS, as tests and
/// tools do, is neither remembered nor skipped.
fn is_the_node_verifier(verifier_index: &VerifierIndex<Fq>, srs: &SRS<Vesta>) -> bool {
    TransactionVerifier::get().is_some_and(|node| std::ptr::eq(&*node, verifier_index))
        && is_the_node_srs(srs)
}

/// [`verify_transaction`] for the transaction proofs this process has not
/// verified. `true` only if every proof verifies, now or before.
pub fn verify_transaction_once(
    proofs: &[(&Statement<SokDigest>, &TransactionSnarkProofStableV2)],
    verifier_index: &VerifierIndex<Fq>,
    srs: &SRS<Vesta>,
) -> bool {
    if !enabled() || !is_the_node_verifier(verifier_index, srs) {
        return verify_transaction(proofs.iter().copied(), verifier_index, srs);
    }
    verify_transaction_with(&NODE, proofs, verifier_index, srs)
}

/// As [`verify_transaction_once`], with these verified proofs, which must
/// be used with one verifier index and SRS only: neither is in the key.
pub(super) fn verify_transaction_with(
    verified: &VerifiedProofs,
    proofs: &[(&Statement<SokDigest>, &TransactionSnarkProofStableV2)],
    verifier_index: &VerifierIndex<Fq>,
    srs: &SRS<Vesta>,
) -> bool {
    use rayon::prelude::*;

    let keys = keys_of_batch(|| {
        proofs
            .par_iter()
            .map(|&(statement, proof)| VerifiedProof::transaction(statement, proof))
            .collect()
    });
    let Some(keys) = keys else {
        return verify_transaction(proofs.iter().copied(), verifier_index, srs);
    };
    let items = keys.into_iter().zip(proofs.iter().copied()).collect();
    verified.verify_once(items, |unverified| {
        verify_transaction(unverified.iter().copied(), verifier_index, srs)
    })
}

/// [`verify_zkapps`] for the zkApp proofs this process has not verified.
/// `true` only if every proof verifies, now or before.
pub fn verify_zkapps_once(proofs: &[ZkappProofToVerify], srs: &SRS<Vesta>) -> bool {
    if !enabled() || !is_the_node_srs(srs) {
        return verify_zkapps(proofs, srs);
    }
    verify_zkapps_with(&NODE, proofs, srs)
}

/// As [`verify_zkapps_once`], with these verified proofs, which must be
/// used with one SRS only: it is not in the key.
fn verify_zkapps_with(
    verified: &VerifiedProofs,
    proofs: &[ZkappProofToVerify],
    srs: &SRS<Vesta>,
) -> bool {
    use rayon::prelude::*;

    let keys = keys_of_batch(|| {
        // Each distinct key digested once: a block's proofs share a few keys.
        let mut keys: Vec<(&VerificationKey, [u8; 32])> = Vec::new();
        let mut key_digests = Vec::with_capacity(proofs.len());
        for &(key, _, _) in proofs {
            let digest = match keys.iter().find(|(k, _)| *k == key) {
                Some((_, digest)) => *digest,
                None => {
                    let digest = verification_key_digest(key)?;
                    keys.push((key, digest));
                    digest
                }
            };
            key_digests.push(digest);
        }
        proofs
            .par_iter()
            .zip(&key_digests)
            .map(|(&(_, statement, proof), key_digest)| {
                VerifiedProof::zkapp(key_digest, statement, proof)
            })
            .collect()
    });
    let Some(keys) = keys else {
        return verify_zkapps(proofs, srs);
    };
    let items = keys.into_iter().zip(proofs.iter().copied()).collect();
    verified.verify_once(items, |unverified| verify_zkapps(unverified, srs))
}

/// Puts a key in the node's set without verifying anything, which no
/// verification can do: for tests that tell skipping from verifying.
#[cfg(test)]
pub(crate) fn remember_unverified(proof: VerifiedProof) {
    NODE.held().add(proof);
}

/// Held by tests that turn skipping off, or that need it on, while they run:
/// the switch is the process's.
#[cfg(test)]
pub(crate) fn test_serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u64) -> VerifiedProof {
        VerifiedProof(digest(b"test", &[&n.to_le_bytes()]))
    }

    #[test]
    fn a_verified_proof_is_not_verified_again() {
        let verified = VerifiedProofs::new();
        let items = || vec![(key(1), 1u64), (key(2), 2), (key(3), 3)];

        let mut seen = Vec::new();
        assert!(verified.verify_once(items(), |unverified| {
            seen = unverified.to_vec();
            true
        }));
        assert_eq!(seen, [1, 2, 3]);

        // All three are known: `verify` is not called.
        assert!(verified.verify_once(items(), |_| panic!("verified again")));

        // A new proof among known ones is the only one verified.
        let mut seen = Vec::new();
        let mut more = items();
        more.insert(1, (key(4), 4));
        assert!(verified.verify_once(more, |unverified| {
            seen = unverified.to_vec();
            true
        }));
        assert_eq!(seen, [4]);
        assert_eq!(
            verified.stats(),
            Stats {
                skipped: 6,
                verified: 4,
                held: 4
            }
        );
    }

    #[test]
    fn a_refused_batch_is_not_remembered() {
        let verified = VerifiedProofs::new();
        let items = || vec![(key(10), 10u64), (key(11), 11)];

        assert!(!verified.verify_once(items(), |_| false));
        assert_eq!(verified.stats().held, 0);
        let mut seen = Vec::new();
        assert!(verified.verify_once(items(), |unverified| {
            seen = unverified.to_vec();
            true
        }));
        assert_eq!(seen, [10, 11]);

        // A refused proof next to known ones is refused, and they stay known.
        let mut with_bad = items();
        with_bad.push((key(12), 12));
        let mut seen = Vec::new();
        assert!(!verified.verify_once(with_bad, |unverified| {
            seen = unverified.to_vec();
            false
        }));
        assert_eq!(seen, [12]);
        assert!(verified.verify_once(items(), |_| panic!("verified again")));
        assert!(!verified.verify_once(vec![(key(12), 12u64)], |_| false));
        assert_eq!(verified.stats().held, 2);
    }

    #[test]
    fn a_panic_while_verifying_remembers_nothing() {
        let verified = VerifiedProofs::new();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verified.verify_once(vec![(key(40), 40u64)], |_| panic!("a failed check"))
        }));
        assert!(panicked.is_err());
        assert_eq!(verified.stats().held, 0);
        assert!(!verified.verify_once(vec![(key(40), 40u64)], |_| false));
    }

    /// Five real zkApp proofs with their keys and statements (see
    /// `zkapp_batch_tests` in `proofs::verification`).
    const ZKAPP_PROOFS: &[u8] = include_bytes!("../proofs/data/zkapp_proofs_batch.bin");

    type Zkapp = (
        VerificationKey,
        ZkappStatement,
        PicklesProofProofsVerified2ReprStableV2,
    );

    fn zkapps() -> Vec<Zkapp> {
        use mina_p2p_messages::{binprot::BinProtRead, v2};

        Vec::<(
            v2::MinaBaseVerificationKeyWireStableV1,
            v2::MinaBaseZkappStatementStableV2,
            v2::PicklesProofProofsVerified2ReprStableV2,
        )>::binprot_read(&mut &*ZKAPP_PROOFS)
        .unwrap()
        .iter()
        .map(|(key, statement, proof)| {
            (
                key.try_into().unwrap(),
                statement.try_into().unwrap(),
                proof.clone(),
            )
        })
        .collect()
    }

    fn batch(items: &[Zkapp]) -> Vec<ZkappProofToVerify<'_>> {
        items
            .iter()
            .map(|(key, statement, proof)| (key, statement, proof))
            .collect()
    }

    fn srs() -> std::sync::Arc<SRS<Vesta>> {
        crate::verifier::get_srs::<Fp>()
    }

    fn plus_one(value: &mina_p2p_messages::bigint::BigInt) -> mina_p2p_messages::bigint::BigInt {
        (value.to_field::<Fp>().unwrap() + Fp::from(1u64)).into()
    }

    /// Real zkApp proofs are verified once, and what is skipped is only the
    /// exact key, statement and proof verified before.
    #[test]
    fn zkapp_proofs_are_verified_once_and_any_change_is_verified() {
        let verified = VerifiedProofs::new();
        let items = zkapps();
        let srs = srs();
        assert_ne!(items[0].0, items[4].0, "the fixture has two keys");

        assert!(verify_zkapps_with(&verified, &batch(&items[..3]), &srs));
        assert_eq!(verified.stats().verified, 3);
        assert_eq!(verified.stats().skipped, 0);
        // The first three again, then all five: two left to verify.
        assert!(verify_zkapps_with(&verified, &batch(&items[..3]), &srs));
        assert!(verify_zkapps_with(&verified, &batch(&items), &srs));
        assert_eq!(
            verified.stats(),
            Stats {
                skipped: 6,
                verified: 5,
                held: 5
            }
        );

        // A verified proof with another statement is verified, and refused.
        let mut changed = items.clone();
        changed[0].1.account_update.0 += Fp::from(1u64);
        assert!(!verify_zkapps_with(&verified, &batch(&changed), &srs));
        let mut changed = items.clone();
        changed[1].1.calls.0 += Fp::from(1u64);
        assert!(!verify_zkapps_with(&verified, &batch(&changed[1..2]), &srs));

        // A verified statement with a changed proof.
        let mut changed = items.clone();
        changed[2].2.prev_evals.ft_eval1 = plus_one(&changed[2].2.prev_evals.ft_eval1);
        assert!(!verify_zkapps_with(&verified, &batch(&changed), &srs));

        // A verified proof and statement under the other key.
        let mut changed = items.clone();
        changed[0].0 = items[4].0.clone();
        assert!(!verify_zkapps_with(&verified, &batch(&changed[..1]), &srs));

        // Two verified proofs with their statements swapped.
        let mut changed = items.clone();
        let statement = changed[0].1.clone();
        changed[0].1 = changed[1].1.clone();
        changed[1].1 = statement;
        assert!(!verify_zkapps_with(&verified, &batch(&changed), &srs));

        // None of the refused ones was remembered, and the five still are.
        assert_eq!(verified.stats().held, 5);
        assert_eq!(verified.stats().verified, 5);
        let skipped = verified.stats().skipped;
        assert!(verify_zkapps_with(&verified, &batch(&items), &srs));
        assert_eq!(verified.stats().skipped, skipped + 5);
    }

    /// Every part of what a zkApp proof is verified with is in its key.
    #[test]
    fn a_zkapp_key_covers_the_key_the_statement_and_the_proof() {
        use crate::ProofVerified;

        let items = zkapps();
        let key_of = |(key, statement, proof): &Zkapp| {
            VerifiedProof::zkapp(&verification_key_digest(key).unwrap(), statement, proof).unwrap()
        };
        let original = key_of(&items[0]);
        assert!(original == key_of(&items[0].clone()));

        let mut other = items[0].clone();
        other.0 = items[4].0.clone();
        assert!(original != key_of(&other));
        // Keys that differ only in what they say of their own shape.
        let another = |n: &ProofVerified| match n {
            ProofVerified::N2 => ProofVerified::N1,
            _ => ProofVerified::N2,
        };
        let mut other = items[0].clone();
        other.0.max_proofs_verified = another(&other.0.max_proofs_verified);
        assert!(original != key_of(&other));
        let mut other = items[0].clone();
        other.0.actual_wrap_domain_size = another(&other.0.actual_wrap_domain_size);
        assert!(original != key_of(&other));

        let mut other = items[0].clone();
        other.1.account_update.0 += Fp::from(1u64);
        assert!(original != key_of(&other));
        let mut other = items[0].clone();
        other.1.calls.0 += Fp::from(1u64);
        assert!(original != key_of(&other));
        // The two parts of a statement exchanged.
        let mut other = items[0].clone();
        std::mem::swap(&mut other.1.account_update, &mut other.1.calls);
        assert!(original != key_of(&other));

        let mut other = items[0].clone();
        other.2.prev_evals.ft_eval1 = plus_one(&other.2.prev_evals.ft_eval1);
        assert!(original != key_of(&other));
        let mut other = items[0].clone();
        other.2.proof.ft_eval1 = plus_one(&other.2.proof.ft_eval1);
        assert!(original != key_of(&other));
    }

    /// `MINA_VERIFY_EVERY_PROOF`, or `set_enabled(false)`, verifies every
    /// proof. To tell skipping from verifying, the node's set is given the
    /// key of a proof that does not verify, which no verification can do.
    #[test]
    fn nothing_is_skipped_when_turned_off() {
        let _serial = test_serial();
        let mut items = zkapps();
        items.truncate(1);
        items[0].1.calls.0 += Fp::from(20_261_117u64);
        let srs = srs();
        assert!(!verify_zkapps(&batch(&items), &srs));

        let (key, statement, proof) = &items[0];
        let forged =
            VerifiedProof::zkapp(&verification_key_digest(key).unwrap(), statement, proof).unwrap();
        remember_unverified(forged);
        // Whatever the environment says: the switch is restored below.
        let was_enabled = enabled();
        set_enabled(true);
        let skipped = verify_zkapps_once(&batch(&items), &srs);
        set_enabled(false);
        let refused = !verify_zkapps_once(&batch(&items), &srs);
        set_enabled(was_enabled);
        assert!(skipped && refused);

        // Another SRS than the node's: verified, whatever the set holds.
        let other_srs = SRS::<Vesta>::clone(&srs);
        set_enabled(true);
        let refused = !verify_zkapps_once(&batch(&items), &other_srs);
        set_enabled(was_enabled);
        assert!(refused);
    }

    /// A proof whose encoder refuses it (an array of 17 evaluations where
    /// the type allows 16) has no key. The verifier gets the batch and
    /// refuses it, as it did before proofs were remembered: no panic.
    #[test]
    fn a_proof_that_cannot_be_encoded_is_refused_without_a_panic() {
        let verified = VerifiedProofs::new();
        let items = zkapps();
        let srs = srs();
        let mut bad = items[0].clone();
        let entry = bad.2.prev_evals.evals.evals.w[0].0[0].clone();
        bad.2.prev_evals.evals.evals.w.0[0].0 = vec![entry; 17].into();
        // The round count is refused before any expensive check.
        bad.2.proof.bulletproof.lr = vec![].into();
        let key = verification_key_digest(&bad.0).unwrap();
        assert!(VerifiedProof::zkapp(&key, &bad.1, &bad.2).is_none());

        assert!(!verify_zkapps(&batch(std::slice::from_ref(&bad)), &srs));
        assert!(!verify_zkapps_with(
            &verified,
            &batch(std::slice::from_ref(&bad)),
            &srs
        ));
        // With valid proofs around it, verified before or not.
        assert!(verify_zkapps_with(&verified, &batch(&items[1..3]), &srs));
        let mut mixed = items.clone();
        mixed[0] = bad;
        assert!(!verify_zkapps_with(&verified, &batch(&mixed), &srs));
        assert_eq!(verified.stats().held, 2);
        assert!(verify_zkapps_with(&verified, &batch(&items[1..3]), &srs));
    }

    /// A panic while the keys of a batch are made is contained: the batch
    /// has no keys.
    #[test]
    fn a_panic_making_keys_is_contained() {
        assert!(keys_of_batch(|| panic!("a malformed proof")).is_none());
        assert!(keys_of_batch(|| None).is_none());
        assert_eq!(keys_of_batch(|| Some(Vec::new())).map(|k| k.len()), Some(0));
    }

    #[test]
    fn only_the_node_verifier_and_srs_use_the_node_set() {
        let srs = srs();
        let other_srs = SRS::<Vesta>::clone(&srs);
        assert!(is_the_node_srs(&srs));
        assert!(!is_the_node_srs(&other_srs));

        let other_index = crate::proofs::verifiers::mainnet_transaction_verifier_index();
        assert!(!is_the_node_verifier(&other_index, &srs));
        // The node's own, as the node makes it and as the block path reads
        // it (`VERIFIER_INDEX`), however many times it is cloned.
        let node = TransactionVerifier::make();
        assert!(is_the_node_verifier(&node, &srs));
        assert!(is_the_node_verifier(&node.clone(), &srs));
        assert!(is_the_node_verifier(&super::super::VERIFIER_INDEX, &srs));
        assert!(!is_the_node_verifier(&node, &other_srs));
        // A copy of the node's index is another index.
        let copy = VerifierIndex::<Fq>::clone(&node);
        assert!(!is_the_node_verifier(&copy, &srs));
    }

    #[test]
    fn the_oldest_proofs_are_dropped_at_capacity() {
        let mut held = Held {
            proofs: HashSet::new(),
            order: VecDeque::new(),
        };
        let last = CAPACITY as u64 + 9;
        for n in 0..=last {
            held.add(key(n));
            // Adding a held proof again neither duplicates nor renews it.
            held.add(key(n));
        }
        assert_eq!(held.order.len(), CAPACITY);
        assert_eq!(held.proofs.len(), CAPACITY);
        assert!(!held.proofs.contains(&key(0)));
        assert!(!held.proofs.contains(&key(9)));
        assert!(held.proofs.contains(&key(10)));
        assert!(held.proofs.contains(&key(last)));
    }

    #[test]
    fn the_parts_of_a_key_do_not_run_together() {
        assert_ne!(digest(b"a", &[b"bc", b"d"]), digest(b"a", &[b"b", b"cd"]));
        assert_ne!(digest(b"a", &[b"bc"]), digest(b"ab", &[b"c"]));
        assert_ne!(digest(b"a", &[b"b", b""]), digest(b"a", &[b"b"]));
    }
}
