//! SNARK work the snark pool verified is not verified again in a block, on
//! the paths and with the verifier index and SRS a node uses.
//!
//! This is its own process: it sets the network to mainnet, whose transaction
//! verification key the work was made for, and it reads the process's counts
//! of verified proofs. The unit tests of `verifier::verified_proofs` cover
//! the set itself, with its own index.
//!
//!   cargo test --release -p mina-tree --test verify_proofs_once

use mina_core::snark::Snark;
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::BinProtRead,
    v2::{
        LedgerProofProdStableV2, TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs,
    },
};
use mina_signer::CompressedPubKey;
use mina_tree::{
    proofs::verifiers::TransactionVerifier,
    scan_state::{
        currency::Fee,
        scan_state::transaction_snark::{LedgerProof, SokMessage},
    },
    verifier::{
        get_srs,
        snark_work::{verify_snark_works, SnarkWorkError},
        verified_proofs::{self, Stats},
        Verifier,
    },
};

/// Real Mesa mainnet work, from precomputed blocks: two base proofs, one
/// merge proof, and a merge and a base proof.
const WORKS: &[&[u8]] = &[
    include_bytes!("../src/verifier/fixtures/mainnet-551267-1.bin"),
    include_bytes!("../src/verifier/fixtures/mainnet-556667-40.bin"),
    include_bytes!("../src/verifier/fixtures/mainnet-548252-46.bin"),
];
const PROOFS: u64 = 5;

fn works() -> Vec<TransactionSnarkWorkTStableV2> {
    WORKS
        .iter()
        .map(|bytes| TransactionSnarkWorkTStableV2::binprot_read(&mut &bytes[..]).unwrap())
        .collect()
}

fn ledger_proofs(work: &TransactionSnarkWorkTStableV2) -> Vec<&LedgerProofProdStableV2> {
    match &work.proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
    }
}

/// The works as a block's completed works reach the verifier, with this fee.
fn completed(
    works: &[TransactionSnarkWorkTStableV2],
    fee: impl Fn(&TransactionSnarkWorkTStableV2) -> Fee,
) -> Vec<(LedgerProof, SokMessage)> {
    works
        .iter()
        .flat_map(|work| {
            let fee = fee(work);
            let prover: CompressedPubKey = (&work.prover).try_into().unwrap();
            ledger_proofs(work).into_iter().map(move |proof| {
                (
                    LedgerProof::try_from(proof).unwrap(),
                    SokMessage::create(fee, prover.clone()),
                )
            })
        })
        .collect()
}

fn own_fee(work: &TransactionSnarkWorkTStableV2) -> Fee {
    Fee::from(&work.fee)
}

/// The first work with one evaluation of its first proof changed.
fn tampered() -> Vec<TransactionSnarkWorkTStableV2> {
    let mut works = works();
    let proof = match &mut works[0].proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => p,
        TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => p,
    };
    let mut inner = (*proof.0.proof.get().unwrap()).clone();
    let ft_eval1: Fp = inner.0.proof.ft_eval1.to_field().unwrap();
    inner.0.proof.ft_eval1 = (ft_eval1 + Fp::from(1u64)).into();
    proof.0.proof = inner.into();
    works
}

#[test]
fn work_the_pool_verified_is_not_verified_again_in_a_block() {
    mina_core::NetworkConfig::init("mainnet").unwrap();
    // As a node makes them: its builder passes these to the snark pool's
    // verification, and the block path reads the same two.
    let index = TransactionVerifier::make();
    let srs = get_srs::<Fp>();
    // Whatever the environment says.
    verified_proofs::set_enabled(true);
    let since = |before: Stats| {
        let now = verified_proofs::stats();
        (now.skipped - before.skipped, now.verified - before.verified)
    };

    // The snark pool verifies the work as it arrives.
    let before = verified_proofs::stats();
    let snarks: Vec<Snark> = works().into_iter().map(Snark::from).collect();
    assert_eq!(verify_snark_works(&snarks, &index, &srs), Ok(()));
    assert_eq!(since(before), (0, PROOFS));
    assert_eq!(verified_proofs::stats().held as u64, PROOFS);

    // A block buys it: none of its proofs is verified again.
    let before = verified_proofs::stats();
    assert_eq!(
        Verifier.verify_transaction_snarks(completed(&works(), own_fee)),
        Ok(())
    );
    assert_eq!(since(before), (PROOFS, 0));

    // And the pool is offered it again.
    let before = verified_proofs::stats();
    assert_eq!(verify_snark_works(&snarks, &index, &srs), Ok(()));
    assert_eq!(since(before), (PROOFS, 0));

    // The same proofs for another fee: refused by the sok digest, in a
    // block and in the pool, although every proof is known.
    let other_fee =
        |work: &TransactionSnarkWorkTStableV2| Fee::from_u64(Fee::from(&work.fee).as_u64() + 1);
    assert_eq!(
        Verifier.verify_transaction_snarks(completed(&works(), other_fee)),
        Err("Transaction_snark.verify: Mismatched sok_message".to_string())
    );
    let mut readvertised = works();
    readvertised[0].fee = (&other_fee(&readvertised[0])).into();
    let readvertised: Vec<Snark> = readvertised.into_iter().map(Snark::from).collect();
    assert_eq!(
        verify_snark_works(&readvertised, &index, &srs),
        Err(SnarkWorkError::SokDigestMismatch { work: 0 })
    );

    // A changed proof for a known statement is verified, and refused, in a
    // block and in the pool; the four unchanged proofs are skipped.
    let before = verified_proofs::stats();
    assert_eq!(
        Verifier.verify_transaction_snarks(completed(&tampered(), own_fee)),
        Err("Transaction_snark.verify: verification failed".to_string())
    );
    let bad: Vec<Snark> = tampered().into_iter().map(Snark::from).collect();
    assert_eq!(
        verify_snark_works(&bad, &index, &srs),
        Err(SnarkWorkError::VerificationFailed)
    );
    assert_eq!(since(before), (2 * (PROOFS - 1), 0));
    assert_eq!(verified_proofs::stats().held as u64, PROOFS);

    // Turned off, every proof is verified and nothing is counted.
    verified_proofs::set_enabled(false);
    let before = verified_proofs::stats();
    assert_eq!(
        Verifier.verify_transaction_snarks(completed(&works(), own_fee)),
        Ok(())
    );
    assert_eq!(
        Verifier.verify_transaction_snarks(completed(&tampered(), own_fee)),
        Err("Transaction_snark.verify: verification failed".to_string())
    );
    assert_eq!(since(before), (0, 0));
    verified_proofs::set_enabled(true);
}
