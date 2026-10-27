//! The zkApp step circuits on Mesa (P5.1): real Mesa zkApp command base
//! jobs, proved segment by segment and merged, as a snark worker proves
//! `Transition` work, with the Mesa circuit blobs.
//!
//! The fixtures are OCaml's own jobs, written by
//! `tools/block-replay/examples/mesa_zkapp_job_fixtures.rs` from a replay that
//! checks every staged ledger hash, so their statements are the ones in the
//! chain's scan state. Next to each is the snark work an OCaml worker
//! produced for it, from a later block: the sok message and the proof. A
//! Rust proof of the job for the same sok message must have the statement,
//! sok digest included, of OCaml's proof, and verify with the transaction
//! verification key the network uses.

use mina_p2p_messages::binprot::BinProtRead;
use mina_signer::CompressedPubKey;

use super::*;
use crate::{
    proofs::{
        provers::{init_devnet_with_mesa_blobs, init_mainnet_with_mesa_blobs, ZkappProver},
        transaction::mesa_tests::{Outcome, PROVING},
        verification::verify_transaction,
        verifiers::{transaction_verifier_index_of, TransactionVerifier},
        VerifierIndex,
    },
    scan_state::currency::{Fee, Magnitude},
};

/// A base job as a snark worker gets it for `Transition` work.
type Fixture = (
    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    v2::TransactionWitnessStableV2,
);

/// The snark work OCaml produced for a job: its sok message and proof.
type OcamlWork = (v2::MinaBaseSokMessageStableV1, v2::LedgerProofProdStableV2);

fn fixture(bytes: &[u8]) -> Fixture {
    Fixture::binprot_read(&mut &*bytes).unwrap()
}

fn ocaml_work(bytes: &[u8]) -> (SokMessage, v2::LedgerProofProdStableV2) {
    let (message, proof) = OcamlWork::binprot_read(&mut &*bytes).unwrap();
    ((&message).try_into().unwrap(), proof)
}

/// The authorization of each account update, fee payer first, as the
/// fixture extractor names shapes: `S` signature, `N` none, `P` proof.
fn shape(zkapp_command: &ZkAppCommand) -> String {
    let mut shape = String::from("S");
    zkapp_command.account_updates.fold((), |(), update| {
        shape.push(match update.authorization {
            Control::Signature(_) => 'S',
            Control::NoneGiven => 'N',
            Control::Proof(_) => 'P',
        });
    });
    shape
}

/// Whether `verifier` accepts `proof` for `statement`, as the snark pool
/// checks work.
fn verifies(
    statement: &Statement<SokDigest>,
    proof: &v2::TransactionSnarkProofStableV2,
    verifier: &VerifierIndex<Fq>,
) -> bool {
    let srs = crate::verifier::get_srs::<Fp>();
    verify_transaction([(statement, proof)], verifier, &srs)
}

/// The other network's embedded transaction verifier.
fn other_network_verifier() -> (&'static str, VerifierIndex<Fq>) {
    let other = match mina_core::NetworkConfig::global().name {
        "devnet" => "mainnet",
        _ => "devnet",
    };
    (other, transaction_verifier_index_of(other).unwrap())
}

/// Each segment of the job against its step circuit, with
/// `only_verify_constraints`: its kind and the outcome.
fn check_segments(
    (statement, tx_witness): &Fixture,
    provers: &ZkappProver,
) -> Vec<(SegmentBasic, Outcome)> {
    let zkapp_command = zkapp_command_of_witness(tx_witness).unwrap();
    let segments = zkapp_command_segments(statement, tx_witness, &zkapp_command).unwrap();
    segments
        .into_iter()
        .enumerate()
        .map(|(i, (witness, spec, statement))| {
            let side_loaded = match spec {
                SegmentBasic::Proved => Some(snapp_proof_data(&witness).unwrap()),
                SegmentBasic::OptSignedOptSigned | SegmentBasic::OptSigned => None,
            };
            if let Some((_, vk)) = side_loaded {
                eprintln!(
                    "segment {i}: side-loaded key with max_proofs_verified {:?}, \
                     actual_wrap_domain_size {:?}",
                    vk.max_proofs_verified, vk.actual_wrap_domain_size
                );
            }
            let started = std::time::Instant::now();
            let outcome = Outcome::of(of_zkapp_command_segment(
                statement,
                &witness,
                &spec,
                &provers.step_opt_signed_opt_signed_prover,
                &provers.step_opt_signed_prover,
                &provers.step_proof_prover,
                &provers.tx_wrap_prover,
                None,
                None,
                true,
            ));
            eprintln!(
                "segment {i} {spec:?}: {outcome:?} in {:.1?}",
                started.elapsed()
            );
            (spec, outcome)
        })
        .collect()
}

/// The circuits reject what they should: the job with its fee payer's
/// signature changed fails its first segment, whose circuit checks it, and
/// the first segment with a statement that claims another target ledger
/// fails too.
fn check_rejections(fixture: &Fixture, provers: &ZkappProver) {
    let mut other_signature = fixture.clone();
    {
        let v2::MinaTransactionTransactionStableV2::Command(command) =
            &mut other_signature.1.transaction
        else {
            panic!("not a command")
        };
        let v2::MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = &mut **command else {
            panic!("not a zkApp command")
        };
        let signature = zkapp.fee_payer.authorization.inner().clone();
        let s: Fq = signature.1.to_field().unwrap();
        zkapp.fee_payer.authorization =
            v2::MinaBaseSignatureStableV1(signature.0, (s + Fq::from(1u64)).into()).into();
    }
    // The witness fails the circuit's assertion that the signature verifies
    // before any constraint is checked, as OCaml's prover refuses it too.
    let outcomes = check_segments(&other_signature, provers);
    assert!(
        matches!(&outcomes[0].1, Outcome::Other(why) if why.contains("not signed")),
        "another fee payer signature: {:?}",
        outcomes[0]
    );

    let (statement, tx_witness) = fixture;
    let zkapp_command = zkapp_command_of_witness(tx_witness).unwrap();
    let segments = zkapp_command_segments(statement, tx_witness, &zkapp_command).unwrap();
    let (witness, spec, mut statement) = segments.into_iter().next().unwrap();
    statement.target.first_pass_ledger += Fp::from(1u64);
    let outcome = Outcome::of(of_zkapp_command_segment(
        statement,
        &witness,
        &spec,
        &provers.step_opt_signed_opt_signed_prover,
        &provers.step_opt_signed_prover,
        &provers.step_proof_prover,
        &provers.tx_wrap_prover,
        None,
        None,
        true,
    ));
    assert!(
        matches!(outcome, Outcome::NotSatisfied(_)),
        "another target ledger: {outcome:?}"
    );
}

/// The kinds of the job's segments, in order.
fn segment_kinds((statement, tx_witness): &Fixture) -> Vec<String> {
    let zkapp_command = zkapp_command_of_witness(tx_witness).unwrap();
    zkapp_command_segments(statement, tx_witness, &zkapp_command)
        .unwrap()
        .iter()
        .map(|(_, spec, _)| format!("{spec:?}"))
        .collect()
}

/// P5.1's gate for one zkApp command base job:
///
/// - each segment's step witness satisfies its Mesa zkApp step circuit, and
///   has its `AUX_LEN` (a witness of another length fails the check);
/// - the segments prove, step and wrap, and their proofs merge, as a snark
///   worker proves the job;
/// - with `work`, OCaml's snark work for the job: for its sok message, the
///   proof's statement, sok digest included, is OCaml's, and OCaml's proof
///   verifies with the same verifier as ours; without, for our own sok
///   message, its statement is the job's, the scan state's;
/// - the proof verifies with the network's transaction verifier, and not for
///   another sok digest or with the other network's key.
fn prove_and_verify(job: &[u8], work: Option<&[u8]>, segments: &[&str]) {
    let fixture = fixture(job);
    assert_eq!(segment_kinds(&fixture), segments);
    let job_statement = Statement::<()>::try_from(&fixture.0).unwrap();

    let verifier = TransactionVerifier::make();
    let (message, expected) = match work {
        Some(work) => {
            let (message, ocaml_proof) = ocaml_work(work);
            let ocaml_statement =
                Statement::<SokDigest>::try_from(&ocaml_proof.0.statement).unwrap();
            assert!(
                verifies(
                    &ocaml_statement,
                    &ocaml_proof.0.proof.get().unwrap(),
                    &verifier
                ),
                "OCaml's proof does not verify"
            );
            assert_eq!(ocaml_statement.clone().without_digest(), job_statement);
            (message, ocaml_statement)
        }
        None => {
            let message = SokMessage {
                fee: Fee::from_u64(0),
                prover: CompressedPubKey::from_address(
                    "B62qpK6TcG4sWdtT3BzdbWHiK3RJMj3Zbo9mqwBos7cVsydPMCj5wZx",
                )
                .unwrap(),
            };
            let expected = job_statement.clone().with_digest(message.digest());
            (message, expected)
        }
    };
    let provers = ZkappProver::make(Some(verifier.clone())).unwrap();

    for (i, (spec, outcome)) in check_segments(&fixture, &provers).into_iter().enumerate() {
        assert_eq!(outcome, Outcome::Satisfied, "segment {i}, {spec:?}");
    }
    check_rejections(&fixture, &provers);

    let started = std::time::Instant::now();
    let proof = generate_zkapp_proof(ZkappParams {
        statement: &fixture.0,
        tx_witness: &fixture.1,
        message: &message,
        step_opt_signed_opt_signed_prover: &provers.step_opt_signed_opt_signed_prover,
        step_opt_signed_prover: &provers.step_opt_signed_prover,
        step_proof_prover: &provers.step_proof_prover,
        merge_step_prover: &provers.merge_step_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        opt_signed_path: None,
        proved_path: None,
    })
    .unwrap();
    eprintln!(
        "proved {} segments and {} merges in {:.1?}",
        segments.len(),
        segments.len() - 1,
        started.elapsed()
    );

    assert_eq!(proof.statement, expected);
    let proof = v2::LedgerProofProdStableV2::from(&proof);
    assert!(verifies(
        &expected,
        &proof.0.proof.get().unwrap(),
        &verifier
    ));

    let mut other_sok = expected.clone();
    other_sok.sok_digest = SokMessage {
        fee: message.fee.checked_add(&Fee::from_u64(1)).unwrap(),
        prover: message.prover.clone(),
    }
    .digest();
    assert!(!verifies(
        &other_sok,
        &proof.0.proof.get().unwrap(),
        &verifier
    ));
    let (other, other_verifier) = other_network_verifier();
    assert!(
        !verifies(&expected, &proof.0.proof.get().unwrap(), &other_verifier),
        "the proof verifies with the {other} key too"
    );
}

/// The fixtures, OCaml's own base jobs, from the replays that wrote them
/// (`mesa_zkapp_job_fixtures`), each with the shape its name says. Mainnet
/// block 548185 (3NKrDP9Sp2e9QjRDMYPuG8nsQwto4u5NwBreZ64fyhNtNTGGiN2i), the
/// first zkApp command after the fork: 5JtzL5AeHMWgAVVoBtVz6L9XeJa4Qnxd1QzrdGJHyME9Yk6AdNbz
/// from B62qpjxUpgdjzwQfd8q2gzxi99wN7SCgmofpvw27MBkfNHfHoY2VH32, nonce 1261,
/// five account updates, the fourth authorised by a proof, global slot
/// 958535; OCaml's work for it in block 548252.
const MAINNET_PROVED: &[u8] = include_bytes!("../data/mesa_mainnet_zkapp_proved_base_job.bin");
const MAINNET_PROVED_WORK: &[u8] =
    include_bytes!("../data/mesa_mainnet_zkapp_proved_ocaml_work.bin");
/// Mainnet block 548186 (3NKFRFX8P4ZB4sdhZ7tEvLNRzbYKrQZAwXBqZ7JDXq7PUAjwatWx):
/// 5JuMqnrRz4HrKXo4hZYcuoGxiZnmYKDeMt8PGGsTVNAGajGPuh4X, the same fee payer,
/// nonce 1262, six account updates, two proved ones in a row, global slot
/// 958537; OCaml's work in block 548252.
const MAINNET_TWO_PROOFS: &[u8] =
    include_bytes!("../data/mesa_mainnet_zkapp_two_proofs_base_job.bin");
const MAINNET_TWO_PROOFS_WORK: &[u8] =
    include_bytes!("../data/mesa_mainnet_zkapp_two_proofs_ocaml_work.bin");
/// Mainnet block 548945 (3NKLfCkJrnpPN8ZWVD5a46PtfJ4RANXJVH7w1Hn8ri1nP2xMMJgm),
/// the first zkApp command without a proof after the fork:
/// 5JtisMZmd3cneCpxGGCeTe6YSvhx33NJ2o2iSw1j8C6mF5gNaizU from
/// B62qpKnbJak5bAHNuG8dWNjiun1qnDp2q5uPBvZ5FiZtqFCtztd16Wv, nonce 10, a signed
/// and an unauthorised account update, global slot 960345; OCaml's work in
/// block 549051.
const MAINNET_SIGNED: &[u8] = include_bytes!("../data/mesa_mainnet_zkapp_signed_base_job.bin");
const MAINNET_SIGNED_WORK: &[u8] =
    include_bytes!("../data/mesa_mainnet_zkapp_signed_ocaml_work.bin");
/// Devnet block 577542 (3NLthH3htiQwpcHbGqati5pUdtuczte8dXqSUBxNDcfH9phpqMA8):
/// 5JuLmuSMWBRSGQiHY6PZNakBLFsuFAN9YDjXjm64XAdN1AhjMSHn from
/// B62qjxjdkqi5EQnDBAMixjUDMYCrNhLY88EwBuP5s4szGiH9p381QAy, nonce 16805, one
/// proved account update, global slot 919189; OCaml's work in block 577713.
/// From a replay starting at the devnet root 577343, as P4.8's fixtures.
const DEVNET_PROVED: &[u8] = include_bytes!("../data/mesa_devnet_zkapp_proved_base_job.bin");
const DEVNET_PROVED_WORK: &[u8] = include_bytes!("../data/mesa_devnet_zkapp_proved_ocaml_work.bin");
/// Devnet block 577969 (3NLBppScEUoEPmPyeQDToVCCfPG5bKLgAeomqTW6xH8XNQufLDQz):
/// P4.15's command, 5JutrMV8zE62qQoANZzb7YoekYHPuV8BRDhTGoe7k9RnrEHZzkbT, sent
/// through our node: a fee payer, a signed debit and an unauthorised credit,
/// global slot 920033; OCaml's work for it in block 578118
/// (3NKYSPvw1JfVmkS24L6ZrbXGLJqudiQZsmeFvBdCz4BnwYWTtg6H).
const DEVNET_SIGNED: &[u8] = include_bytes!("../data/mesa_devnet_zkapp_signed_base_job.bin");
const DEVNET_SIGNED_WORK: &[u8] = include_bytes!("../data/mesa_devnet_zkapp_signed_ocaml_work.bin");
/// Devnet block 578035 (3NKCL3iBo7z7upz5n4pd4umsGndrcGSeGAJ6eB4MiD8pfpetrkjs):
/// R39's command from the web node, 5JuZTyf5n5cx8M1rufzXk8X9qn1CYBRASSP8igAjgAdGesiTT9us,
/// the same shape, nonce 7, global slot 920150. No OCaml work for it in the
/// blocks to 578137.
const DEVNET_SIGNED_R39: &[u8] =
    include_bytes!("../data/mesa_devnet_zkapp_signed_r39_base_job.bin");
/// Devnet block 578848 (3NKcJ65yhYu3eCL2tY6p2C2RBsABgiopGtYGX5UeZgXQQRHLodVY):
/// 5JuamsZyoL7AYxGVW8BETCqNKZimB9nG9rtSE3Lg4LiveUvCo6rb from
/// B62qpZVK9a4jyHxu9hERUHm3bvr397EgCENfEv4h6TjeWYRQaBTp5MQ, nonce 2, one
/// proved account update, global slot 921804. Its zkApp's circuit uses
/// custom gates: the proof's feature flags enable `range_check0`,
/// `range_check1` and `foreign_field_add`, so it carries eleven optional
/// evaluations and a joint combiner. ZK1: the worker failed such jobs on
/// public devnet with 22 extra step witness values. From a replay of the
/// chain from the devnet root 577343 (`mesa_zkapp_job_fixtures`); OCaml's
/// work for it, 0.01 MINA from
/// B62qpfgnUm7zVqi8MJHNB2m37rtgMNDbFNhC2DpMmmVpQt8x6gKv9Ww, in block 578963
/// (3NKz5KZWv62YgtNuB5wU1RS1YMn4sdryTrwD7WPiMiGYwvM7ngqj).
const DEVNET_PROVED_FEATURES: &[u8] =
    include_bytes!("../data/mesa_devnet_zkapp_proved_features_base_job.bin");
const DEVNET_PROVED_FEATURES_WORK: &[u8] =
    include_bytes!("../data/mesa_devnet_zkapp_proved_features_ocaml_work.bin");

/// Each fixture has the shape its name says, is applied, and OCaml's work
/// for it proves its statement: the same statement but for the sok digest,
/// for the work's sok message. No blobs needed.
#[test]
fn mesa_zkapp_fixtures_are_their_shapes() {
    for (name, job, work, expected) in [
        (
            "mainnet proved",
            MAINNET_PROVED,
            Some(MAINNET_PROVED_WORK),
            "SSSSPS",
        ),
        (
            "mainnet two proofs",
            MAINNET_TWO_PROOFS,
            Some(MAINNET_TWO_PROOFS_WORK),
            "SSPPSNN",
        ),
        (
            "mainnet signed",
            MAINNET_SIGNED,
            Some(MAINNET_SIGNED_WORK),
            "SSN",
        ),
        (
            "devnet proved",
            DEVNET_PROVED,
            Some(DEVNET_PROVED_WORK),
            "SP",
        ),
        (
            "devnet signed",
            DEVNET_SIGNED,
            Some(DEVNET_SIGNED_WORK),
            "SSN",
        ),
        ("devnet signed, R39", DEVNET_SIGNED_R39, None, "SSN"),
        (
            "devnet proved, features",
            DEVNET_PROVED_FEATURES,
            Some(DEVNET_PROVED_FEATURES_WORK),
            "SP",
        ),
    ] {
        let (statement, tx_witness) = fixture(job);
        let zkapp_command = zkapp_command_of_witness(&tx_witness).unwrap();
        assert_eq!(shape(&zkapp_command), expected, "{name}");
        assert_eq!(
            tx_witness.status,
            v2::MinaBaseTransactionStatusStableV2::Applied,
            "{name}"
        );
        if let Some(work) = work {
            let (message, proof) = ocaml_work(work);
            let proved = Statement::<SokDigest>::try_from(&proof.0.statement).unwrap();
            assert_eq!(proved.sok_digest, message.digest(), "{name}");
            assert_eq!(
                proved.without_digest(),
                Statement::<()>::try_from(&statement).unwrap(),
                "{name}"
            );
        }
    }
}

/// Mainnet 548945: a fee payer and a signed account update in an
/// `opt_signed-opt_signed` segment, an unauthorised one in `opt_signed`.
#[test]
#[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_zkapp_signed_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_mainnet_with_mesa_blobs();
    prove_and_verify(
        MAINNET_SIGNED,
        Some(MAINNET_SIGNED_WORK),
        &["OptSignedOptSigned", "OptSigned"],
    );
}

/// Mainnet 548185: two `opt_signed-opt_signed` segments, a `proved` one
/// that verifies the account update's zkApp proof in the circuit, and an
/// `opt_signed` one.
#[test]
#[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_zkapp_proved_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_mainnet_with_mesa_blobs();
    prove_and_verify(
        MAINNET_PROVED,
        Some(MAINNET_PROVED_WORK),
        &[
            "OptSignedOptSigned",
            "OptSignedOptSigned",
            "Proved",
            "OptSigned",
        ],
    );
}

/// Mainnet 548186: two `proved` segments in a row between signed ones.
#[test]
#[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_zkapp_two_proofs_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_mainnet_with_mesa_blobs();
    prove_and_verify(
        MAINNET_TWO_PROOFS,
        Some(MAINNET_TWO_PROOFS_WORK),
        &[
            "OptSignedOptSigned",
            "Proved",
            "Proved",
            "OptSignedOptSigned",
            "OptSigned",
        ],
    );
}

/// Devnet 577542: the fee payer alone in `opt_signed`, then a `proved`
/// segment, with the devnet circuits.
#[test]
#[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
            tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_devnet_zkapp_proved_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_devnet_with_mesa_blobs();
    prove_and_verify(
        DEVNET_PROVED,
        Some(DEVNET_PROVED_WORK),
        &["OptSigned", "Proved"],
    );
}

/// The step witnesses of 577542's two segments, `opt_signed` and a `proved`
/// one whose zkApp proof has every feature flag off, are pinned by hash: ZK1
/// changed how a side-loaded proof's flags, joint combiner and optional
/// evaluations are witnessed, and for such a proof the values had to stay
/// what they were. The same hashes came out of the code before ZK1's fix
/// (its six files reverted to `f7be4a169`) and after it.
#[test]
#[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
            tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_devnet_zkapp_step_witnesses_without_flags_are_pinned() {
    use crate::proofs::transaction::witness_hashes;

    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_devnet_with_mesa_blobs();
    let provers = ZkappProver::make(Some(TransactionVerifier::make())).unwrap();
    witness_hashes::enable();
    let outcomes = check_segments(&fixture(DEVNET_PROVED), &provers);
    let hashes = witness_hashes::take();
    for (i, (spec, outcome)) in outcomes.iter().enumerate() {
        assert_eq!(*outcome, Outcome::Satisfied, "segment {i}, {spec:?}");
    }
    let hashes: Vec<(&str, &str)> = hashes
        .iter()
        .map(|(primary, aux)| (primary.as_str(), aux.as_str()))
        .collect();
    eprintln!("step witness hashes (primary, aux): {hashes:?}");
    assert_eq!(hashes, PINNED_NO_FLAG_STEP_WITNESSES);
}

/// SHA-256 of the primary and auxiliary step witness values of 577542's
/// `opt_signed` and `proved` segments.
const PINNED_NO_FLAG_STEP_WITNESSES: [(&str, &str); 2] = [
    (
        "2cf074305007c6ade7f0a1dfcd26fe760543ccded95f97206d7125e238416d4e",
        "bb7b03fecf4f9636bcb9fbbb07386219bfbcc664000a94b8965b142a5f8305cc",
    ),
    (
        "41689d8b884af213eb2a70c0bd05213584dfa3f20d62da132e155e512e1f6a54",
        "ac55151d3cda4da3a83293b63011ccb9e5fea6f3bdb998295a05f19d2530b80b",
    ),
];

/// Devnet 577969, P4.15's command, and 578035, R39's, with the devnet
/// circuits: a fee payer and a signed debit in `opt_signed-opt_signed`, an
/// unauthorised credit in `opt_signed`. OCaml's work for P4.15's is in block
/// 578118; R39's had not been bought by 578137, so its proof's statement is
/// checked against the scan state's.
#[test]
#[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
            tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_devnet_zkapp_signed_proofs_verify() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_devnet_with_mesa_blobs();
    for (job, work) in [
        (DEVNET_SIGNED, Some(DEVNET_SIGNED_WORK)),
        (DEVNET_SIGNED_R39, None),
    ] {
        prove_and_verify(job, work, &["OptSignedOptSigned", "OptSigned"]);
    }
}

/// The proof authorising 578848's account update is from a circuit with
/// custom gates: its feature flags and the optional evaluations it carries,
/// which the step witness has to lay out as OCaml does whatever they are.
/// No blobs needed.
#[test]
fn mesa_devnet_zkapp_features_fixture_has_custom_gates() {
    let (statement, tx_witness) = fixture(DEVNET_PROVED_FEATURES);
    let zkapp_command = zkapp_command_of_witness(&tx_witness).unwrap();
    let segments = zkapp_command_segments(&statement, &tx_witness, &zkapp_command).unwrap();
    let (witness, spec, _) = &segments[1];
    assert!(matches!(spec, SegmentBasic::Proved));
    let (proof, _vk) = snapp_proof_data(witness).unwrap();
    let plonk = &proof.statement.proof_state.deferred_values.plonk;
    let flags = &plonk.feature_flags;
    assert_eq!(
        [
            flags.range_check0,
            flags.range_check1,
            flags.foreign_field_add,
            flags.foreign_field_mul,
            flags.xor,
            flags.rot,
            flags.lookup,
            flags.runtime_tables,
        ],
        [true, true, true, false, false, false, false, false]
    );
    assert!(plonk.joint_combiner.is_some());
    let evals = &proof.prev_evals.evals.evals;
    let optional = [
        &evals.range_check0_selector,
        &evals.range_check1_selector,
        &evals.foreign_field_add_selector,
        &evals.foreign_field_mul_selector,
        &evals.xor_selector,
        &evals.rot_selector,
        &evals.lookup_aggregation,
        &evals.lookup_table,
        &evals.runtime_lookup_table,
        &evals.runtime_lookup_table_selector,
        &evals.xor_lookup_selector,
        &evals.lookup_gate_lookup_selector,
        &evals.range_check_lookup_selector,
        &evals.foreign_field_mul_lookup_selector,
    ]
    .into_iter()
    .filter(|eval| eval.is_some())
    .count()
        + evals
            .lookup_sorted
            .iter()
            .filter(|eval| eval.is_some())
            .count();
    // Two step witness values each: the 22 extra of ZK1's failures.
    assert_eq!(optional, 11);
}

/// The node's verifier on 578848's command, as `Verifier::verify_commands`
/// checks zkApp commands in the transaction pool and in blocks: the keys come
/// from the job's ledger, `common::check` lists the proofs to verify, and
/// `verify_zkapp` accepts the one from a circuit with custom gates and
/// lookups (its deferred values evaluated with its feature flags, ZK1), and
/// refuses it for another statement or with a changed evaluation. No blobs
/// needed.
#[test]
#[ignore = "sets the devnet network config: run alone or with the devnet tests"]
fn mesa_devnet_zkapp_features_proof_verifies_in_the_verifier() {
    use crate::{
        proofs::verification::verify_zkapp,
        scan_state::transaction_logic::{verifiable, zkapp_command, TransactionStatus, WithStatus},
        verifier::common::{check, CheckResult},
    };

    let _ = mina_core::NetworkConfig::init("devnet");
    assert_eq!(
        mina_core::NetworkConfig::global().name,
        "devnet",
        "another test set the network config first: run this test on its own"
    );
    let (statement, tx_witness) = fixture(DEVNET_PROVED_FEATURES);
    let zkapp_command = zkapp_command_of_witness(&tx_witness).unwrap();
    let segments = zkapp_command_segments(&statement, &tx_witness, &zkapp_command).unwrap();
    let ledger = &segments[1].0.local_state_init.ledger;
    let command = zkapp_command::verifiable::create(&zkapp_command, false, |_, id| {
        ledger
            .get_account_if_present(id)
            .and_then(|account| account.zkapp.as_ref()?.verification_key.clone())
            .ok_or_else(|| format!("no key for {id:?}"))
    })
    .unwrap();
    let CheckResult::ValidAssuming((_, proofs)) = check(WithStatus {
        data: verifiable::UserCommand::ZkAppCommand(Box::new(command)),
        status: TransactionStatus::Applied,
    }) else {
        panic!("the command's signatures and keys check, with one proof to verify")
    };
    assert_eq!(proofs.len(), 1);
    let (vk, statement, proof) = &proofs[0];
    let proof: v2::PicklesProofProofsVerified2ReprStableV2 = (&**proof).into();
    let srs = crate::verifier::get_srs::<Fp>();
    assert!(verify_zkapp(vk, statement, &proof, &srs));

    let mut other_statement = statement.clone();
    other_statement.account_update.0 += Fp::from(1u64);
    assert!(!verify_zkapp(vk, &other_statement, &proof, &srs));

    let mut tampered = proof.clone();
    let ft_eval1: Fp = tampered.prev_evals.ft_eval1.to_field().unwrap();
    tampered.prev_evals.ft_eval1 = (ft_eval1 + Fp::from(1u64)).into();
    assert!(!verify_zkapp(vk, statement, &tampered, &srs));
}

/// Devnet 578848: a `proved` segment verifying a zkApp proof whose circuit
/// uses custom gates and lookups (ZK1). Each segment satisfies its circuit
/// with a witness of its `AUX_LEN`, and for OCaml's sok message the proof has
/// OCaml's statement and verifies with the key OCaml's proof verifies with.
#[test]
#[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
            tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
            then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
fn mesa_devnet_zkapp_proved_features_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_devnet_with_mesa_blobs();
    prove_and_verify(
        DEVNET_PROVED_FEATURES,
        Some(DEVNET_PROVED_FEATURES_WORK),
        &["OptSigned", "Proved"],
    );
}

/// The gate of [`prove_and_verify`] for any job: the fixture in
/// `MINA_ZKAPP_JOB`, OCaml's work for it in `MINA_ZKAPP_WORK` if given, on
/// the network in `MINA_ZKAPP_NETWORK` (default mainnet). For jobs too big to
/// commit, such as the 16-segment mainnet command of block 551243.
#[test]
#[ignore = "needs MINA_ZKAPP_JOB and the Mesa circuit blobs"]
fn zkapp_prove_from_env() {
    let Ok(job) = std::env::var("MINA_ZKAPP_JOB") else {
        eprintln!("MINA_ZKAPP_JOB is not set: nothing to prove");
        return;
    };
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match std::env::var("MINA_ZKAPP_NETWORK").as_deref() {
        Ok("devnet") => init_devnet_with_mesa_blobs(),
        _ => init_mainnet_with_mesa_blobs(),
    };
    let job = std::fs::read(job).unwrap();
    let work = std::env::var("MINA_ZKAPP_WORK")
        .ok()
        .map(|path| std::fs::read(path).unwrap());
    let kinds = segment_kinds(&fixture(&job));
    let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
    prove_and_verify(&job, work.as_deref(), &kinds);
}

/// For debugging a circuit: checks every segment of the base job in
/// `MINA_ZKAPP_JOB` (a fixture file) against its step circuit, on the
/// network in `MINA_ZKAPP_NETWORK` (default mainnet), and prints the
/// outcomes, the failing rows with the witness values they use, without
/// asserting. `MINA_WITNESS_TRACE_AT` prints where given values are pushed.
#[test]
#[ignore = "a debugging aid: needs MINA_ZKAPP_JOB and the Mesa circuit blobs"]
fn zkapp_segments_constraints_from_env() {
    let Ok(job) = std::env::var("MINA_ZKAPP_JOB") else {
        eprintln!("MINA_ZKAPP_JOB is not set: nothing to check");
        return;
    };
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match std::env::var("MINA_ZKAPP_NETWORK").as_deref() {
        Ok("devnet") => init_devnet_with_mesa_blobs(),
        _ => init_mainnet_with_mesa_blobs(),
    };
    let job = std::fs::read(job).unwrap();
    let fixture = fixture(&job);
    let zkapp_command = zkapp_command_of_witness(&fixture.1).unwrap();
    eprintln!(
        "shape {}, segments {:?}",
        shape(&zkapp_command),
        segment_kinds(&fixture)
    );
    let verifier = TransactionVerifier::make();
    let provers = ZkappProver::make(Some(verifier)).unwrap();
    for (i, (spec, outcome)) in check_segments(&fixture, &provers).into_iter().enumerate() {
        eprintln!("RESULT segment {i} {spec:?}: {outcome:?}");
    }
}

/// Constructed coverage, not a failed-command chain fixture (P5.1b).
/// The existing account has no key and is_new=true fails. OCaml therefore
/// rolls back the second pass and need not verify the proof authorization.
fn failed_no_key_fixture(with_zkapp: bool) -> Fixture {
    use crate::{
        scan_state::{
            currency::{Balance, Nonce},
            transaction_logic::{zkapp_command as zk, Memo},
        },
        Account, BaseLedger, Mask, VerificationKeyWire, ZkAppAccount,
    };
    use mina_signer::{Keypair, NetworkId, NonceMode, Signer};
    use rand::{rngs::StdRng, SeedableRng};

    let mut rng = StdRng::seed_from_u64(0x503531);
    let payer = Keypair::rand(&mut rng).unwrap();
    let payer_id = AccountId::create(payer.public.into_compressed(), TokenId::default());
    let account_id = AccountId::create(Account::create().public_key, TokenId::default());
    let mut account = Account::create_with(account_id.clone(), Balance::from_u64(10_000_000_000));
    if with_zkapp {
        account.zkapp = Some(Box::new(ZkAppAccount::default()));
    }
    let mut ledger = Mask::new_unattached(35);
    ledger
        .get_or_create_account(
            payer_id.clone(),
            Account::create_with(payer_id.clone(), Balance::from_u64(10_000_000_000)),
        )
        .unwrap();
    ledger
        .get_or_create_account(account_id.clone(), account)
        .unwrap();
    let first = SparseLedger::of_ledger_subset_exn(ledger, &[payer_id.clone(), account_id.clone()]);

    // Reuse a structurally valid proof; the failed update does not verify it.
    let (_, real_witness) = fixture(MAINNET_PROVED);
    let real = zkapp_command_of_witness(&real_witness).unwrap();
    let proof = real
        .account_updates
        .fold(None, |found, update| {
            found.or_else(|| account_update_proof(update).cloned())
        })
        .unwrap();
    let mut precondition = zk::Account::accept();
    precondition.is_new = zk::OrIgnore::Check(true);
    let update = AccountUpdate {
        body: zk::Body {
            public_key: account_id.public_key,
            token_id: TokenId::default(),
            update: zk::Update::noop(),
            balance_change: Signed::zero(),
            increment_nonce: false,
            events: zk::Events::empty(),
            actions: zk::Actions::empty(),
            call_data: Fp::zero(),
            preconditions: zk::Preconditions {
                network: zk::ZkAppPreconditions::accept(),
                account: zk::AccountPreconditions(precondition),
                valid_while: zk::Numeric::Ignore,
            },
            use_full_commitment: false,
            implicit_account_creation_fee: false,
            may_use_token: zk::MayUseToken::No,
            authorization_kind: zk::AuthorizationKind::Proof(VerificationKeyWire::dummy_hash()),
        },
        authorization: Control::Proof(proof.into()),
    };
    let mut command = ZkAppCommand {
        fee_payer: zk::FeePayer {
            body: zk::FeePayerBody {
                public_key: payer_id.public_key,
                fee: Fee::from_u64(1_000_000),
                valid_until: None,
                nonce: Nonce::from_u32(0),
            },
            authorization: mina_signer::Signature::dummy(),
        },
        account_updates: CallForest::empty().cons(None, update),
        memo: Memo::empty(),
    };
    let (_, full) = crate::generators::zkapp_command_builder::get_transaction_commitments(&command);
    let network = match mina_core::NetworkConfig::global().name {
        "mainnet" => NetworkId::MAINNET,
        _ => NetworkId::TESTNET,
    };
    command.fee_payer.authorization =
        mina_signer::create_kimchi(network).sign(&payer, &full, NonceMode::Chunked);

    let (mut statement, mut tx_witness) = fixture(MAINNET_SIGNED);
    let slot = Slot::from_u32(tx_witness.block_global_slot.as_u32());
    let state_view = protocol_state_body_view(&tx_witness.protocol_state_body).unwrap();
    let mut after_fee = first.copy_content();
    let partial = after_fee
        .apply_zkapp_first_pass_unchecked_with_states(
            &mut vec![],
            slot,
            &state_view,
            Signed::zero(),
            Signed::zero(),
            &first,
            &command,
        )
        .unwrap();
    let connecting = after_fee.merkle_root();
    let applied = after_fee
        .copy_content()
        .apply_zkapp_second_pass_unchecked_with_states(&mut vec![], partial)
        .unwrap();
    let crate::scan_state::transaction_logic::TransactionStatus::Failed(failures) =
        &applied.command.status
    else {
        panic!("constructed precondition failure was applied");
    };
    assert!(failures.iter().flatten().any(|failure| matches!(
        failure,
        TransactionFailure::AccountIsNewPreconditionUnsatisfied
    )));
    tx_witness.transaction = v2::MinaTransactionTransactionStableV2::Command(Box::new(
        v2::MinaBaseUserCommandStableV2::ZkappCommand((&command).into()),
    ));
    tx_witness.first_pass_ledger = (&first).into();
    tx_witness.second_pass_ledger = (&after_fee).into();
    statement.connecting_ledger_left = connecting.into();
    statement.connecting_ledger_right = connecting.into();
    let segments = zkapp_command_segments(&statement, &tx_witness, &command).unwrap();
    assert_eq!(segments.len(), 2);
    let first_statement = segments[0].2.clone().without_digest();
    let last_statement = segments[1].2.clone().without_digest();
    // Expected fields from OCaml's failed-command rules, rather than a
    // golden statement captured from Rust output: retain the charged fee,
    // do not commit the local ledger or supply, and reset the local state.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_logic/zkapp_command_logic.ml#L1972-L2021>
    let expected = Statement {
        source: Registers {
            first_pass_ledger: first.copy_content().merkle_root(),
            second_pass_ledger: connecting,
            pending_coinbase_stack: (&statement.source.pending_coinbase_stack)
                .try_into()
                .unwrap(),
            local_state: LocalState::dummy(),
        },
        target: Registers {
            first_pass_ledger: connecting,
            second_pass_ledger: connecting,
            pending_coinbase_stack: (&statement.target.pending_coinbase_stack)
                .try_into()
                .unwrap(),
            local_state: LocalState::dummy(),
        },
        connecting_ledger_left: connecting,
        connecting_ledger_right: connecting,
        supply_increase: Signed::zero(),
        fee_excess: FeeExcess {
            fee_token_l: TokenId::default(),
            fee_excess_l: Signed::of_unsigned(Fee::from_u64(1_000_000)),
            fee_token_r: TokenId::default(),
            fee_excess_r: Signed::zero(),
        },
        sok_digest: (),
    };
    assert_eq!(first_statement.merge(&last_statement).unwrap(), expected);
    assert!(!segments[1].0.local_state_init.will_succeed);
    tx_witness.status = (&applied.command.status).into();
    ((&expected).into(), tx_witness)
}

#[test]
fn failed_proved_zkapp_without_key_uses_ocaml_dummy() {
    use crate::{account::ProofVerified, proofs::verifiers::wrap_domains};
    for with_zkapp in [false, true] {
        let (statement, tx_witness) = failed_no_key_fixture(with_zkapp);
        let command = zkapp_command_of_witness(&tx_witness).unwrap();
        let mut segments = zkapp_command_segments(&statement, &tx_witness, &command).unwrap();
        let (mut witness, spec, statement) = segments.pop().unwrap();
        assert!(matches!(spec, SegmentBasic::Proved));
        let (_, vk) = snapp_proof_data(&witness).unwrap();
        assert_eq!(vk, *crate::VerificationKey::dummy());
        assert_eq!(vk.max_proofs_verified, ProofVerified::N2);
        assert_eq!(vk.actual_wrap_domain_size, ProofVerified::N2);
        // OCaml dummy's sigma/coefficient/selector commitments are all g.
        let g = &vk.wrap_index.generic;
        assert_eq!(g.to_affine().x, Fp::one());
        assert!(vk.wrap_index.sigma.iter().all(|point| point == g));
        assert!(vk.wrap_index.coefficients.iter().all(|point| point == g));
        for point in [
            &vk.wrap_index.psm,
            &vk.wrap_index.complete_add,
            &vk.wrap_index.mul,
            &vk.wrap_index.emul,
            &vk.wrap_index.endomul_scalar,
        ] {
            assert_eq!(point, g);
        }
        assert_eq!(
            wrap_domains(vk.actual_wrap_domain_size.to_int())
                .h
                .log2_size(),
            15
        );
        let mut w = Witness::new::<StepZkappProvedProof>();
        let (input, must_verify) =
            zkapp_main(statement, &witness, &basic_spec(&spec), &mut w).unwrap();
        assert!(input.is_some());
        assert_eq!(must_verify, Boolean::False);
        assert!(!w.aux().is_empty());
        // A witnessed empty location is also covered by OCaml's get_exn
        // and dummy rule; distinguish it from an absent witness entry.
        let update = first_account_update(&witness).unwrap();
        let id = AccountId::create(update.body.public_key.clone(), update.body.token_id.clone());
        let addr = witness.local_state_init.ledger.find_index_exn(id);
        witness
            .local_state_init
            .ledger
            .set_exn(addr, Box::new(crate::Account::empty()));
        assert_eq!(
            snapp_proof_data(&witness).unwrap().1,
            *crate::VerificationKey::dummy()
        );
        witness.local_state_init.ledger = SparseLedger::create(35, Fp::zero());
        assert!(snapp_proof_data(&witness)
            .unwrap_err()
            .to_string()
            .contains("account missing"));
    }
}

#[test]
#[ignore = "needs the Mesa mainnet circuit blobs"]
fn mesa_failed_proved_zkapp_without_key_proof_verifies() {
    let _turn = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    init_mainnet_with_mesa_blobs();
    let job = failed_no_key_fixture(false);
    let mut bytes = Vec::new();
    mina_p2p_messages::binprot::BinProtWrite::binprot_write(&job, &mut bytes).unwrap();
    prove_and_verify(&bytes, None, &["OptSigned", "Proved"]);
}
