//! `mina_block_replay::verify`, which `verify_every_block` runs over a whole
//! directory, on four Mesa mainnet heights' worth of real blocks:
//!
//! - 548148 `3NKtYZVH…`, the first Mesa block, a child of the Mesa genesis
//!   (`crates/web-verifier/tests/fixtures/mainnet-548148.json`), without
//!   completed work;
//! - 548148 `3NKPoFh4…`, a block of the old Berkeley chain at the same height
//!   (research note R1), which must be excluded, not failed;
//! - 550855 `3NKm1wv7…`, with one completed work of two proofs;
//! - 551056 `3NKYoVGo…`, with one completed work of one proof;
//! - 548215 `3NLRHYJ1…`, with a zkApp command that sets a verification key
//!   and then proves an account update with it.
//!
//! Each block's proof must verify against the embedded mainnet blockchain
//! key, each transaction proof against the embedded mainnet transaction key,
//! the zkApp proof against the key its block sets, and each header's delta
//! block chain proof and body reference must pass the node's checks. Then a
//! changed protocol state, a work's fee changed, a work's two proofs given
//! each other's statements, a changed zkApp statement, a delta block chain
//! proof that doesn't lead to the parent, and a body without its completed
//! work must each be reported as failures. Without the block that sets it,
//! the zkApp key can come from a ledger's account, found by its own hash.
//!
//! The keys come from `BlockVerifier::make()` and `TransactionVerifier::make()`,
//! which use the verifier index cache in `$HOME/.mina/circuit-blobs`. With
//! `--release`, which reuses the tool's build, the tests take about 5 s:
//!
//! ```text
//! cargo test --release -p mina-block-replay --test verify_every_block
//! ```

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use ledger::{
    proofs::verifiers::{BlockVerifier, TransactionVerifier},
    Account, ProofVerified, VerificationKey, VerificationKeyWire, ZkAppAccount,
};
use mina_block_replay::{
    read_block_json,
    verify::{
        check, check_files, check_json, completed_works, list_blocks, verify_zkapp_proofs,
        zkapp_keys_of_accounts, BlockFile, BlockResult, Checks, Keys, Options, Outcome, Summary,
        ZkappResult, MAINNET_MESA_GENESIS,
    },
};
use mina_p2p_messages::{
    number::UInt64,
    v2::{TransactionSnarkWorkTStableV2Proofs, UnsignedExtendedUInt64Int64ForVersionTagsStableV1},
};

const FIRST_MESA: &str = "3NKtYZVHgyWrTzFE8j594Wy6HmFTb9rWw3sfPxZmZgRkrg572bAP";
const BERKELEY_548148: &str = "3NKPoFh4wB6MvvJPTLdfnL3GUNnCB3pLG2AC35cF2pcKfue3cG9s";
const BERKELEY_GENESIS: &str = "3NK4BpDSekaqsG6tx8Nse2zJchRft2JpnbvMiog55WCr5xJZaKeP";
const TWO_PROOFS: &str = "550855-3NKm1wv7kCB1fnWLXNU9THuHuSgpXD7rf4C83jKGvNao7UfdMY9k";
const ONE_PROOF: &str = "551056-3NKYoVGoKeJAi1mQ2yE8p934o4sh7KGJ8oohDDHsNCAMfDhnuZzh";
const ZKAPP: &str = "548215-3NLRHYJ1ZAwZf741aTqUzvRfFrGQZ47anzGttnkyTvbnFw39Kri5";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| {
        mina_core::NetworkConfig::init("mainnet").unwrap();
        mina_core::set_work_dir(std::env::temp_dir().join("verify-every-block-test"));
        Keys {
            block: BlockVerifier::make().into(),
            transaction: TransactionVerifier::make().into(),
            ledger_zkapp: Default::default(),
        }
    })
}

fn options() -> Options {
    Options {
        genesis_state_hash: MAINNET_MESA_GENESIS.to_string(),
        batch: true,
        checks: Checks::ALL,
    }
}

fn checked(outcome: Outcome) -> Box<mina_block_replay::verify::Checked> {
    match outcome {
        Outcome::Checked(c) => c,
        other => panic!("{other:?}"),
    }
}

#[test]
fn file_names() {
    let file = BlockFile::from_path(Path::new(&format!("/x/{TWO_PROOFS}.json.zst"))).unwrap();
    assert_eq!(
        (file.height, file.hash.as_str()),
        (550855, &TWO_PROOFS[7..])
    );
    let file =
        BlockFile::from_path(Path::new(&format!("mainnet-548148-{FIRST_MESA}.json"))).unwrap();
    assert_eq!((file.height, file.hash.as_str()), (548148, FIRST_MESA));
    assert!(BlockFile::from_path(Path::new("chain.txt")).is_none());
    assert!(BlockFile::from_path(Path::new("mainnet-548148.json")).is_none());
}

#[test]
fn verifies_mesa_blocks_and_excludes_the_old_chain() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        format!("548148-{BERKELEY_548148}.json.zst"),
        format!("{TWO_PROOFS}.json.zst"),
        format!("{ONE_PROOF}.json.zst"),
        format!("{ZKAPP}.json.zst"),
    ] {
        std::fs::copy(fixtures().join(&name), dir.path().join(&name)).unwrap();
    }
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/web-verifier/tests/fixtures/mainnet-548148.json"),
        dir.path().join(format!("548148-{FIRST_MESA}.json")),
    )
    .unwrap();

    let files = list_blocks(dir.path()).unwrap();
    let results = check_files(&files, keys(), &options(), |_| {});
    let summary = Summary::new(MAINNET_MESA_GENESIS, &results);
    println!("{summary}");

    assert_eq!(summary.files, 5);
    assert_eq!(summary.checked, 4);
    assert_eq!(summary.heights, Some((548148, 551056)));
    let other: Vec<_> = summary.other_chains.iter().collect();
    assert_eq!(
        other,
        [(
            &(BERKELEY_GENESIS.to_string(), "3.0.0".to_string()),
            &vec![(548148, BERKELEY_548148.to_string())]
        )]
    );
    assert_eq!(
        (summary.block_proofs_verified, summary.block_proofs_failed),
        (4, 0)
    );
    assert_eq!(
        (summary.delta_chains_ok, summary.delta_chains_failed),
        (4, 0)
    );
    assert_eq!(
        (summary.body_references_ok, summary.body_references_failed),
        (4, 0)
    );
    assert_eq!((summary.zkapp_keys, summary.zkapp_distinct_keys), (1, 1));
    assert_eq!(summary.blocks_with_zkapp_proofs, 1);
    assert_eq!(
        (summary.zkapp_proofs_verified, summary.zkapp_proofs_failed),
        (1, 0)
    );
    assert_eq!((summary.blocks_with_work, summary.works), (2, 2));
    assert_eq!((summary.proofs_verified, summary.proofs_failed), (3, 0));
    assert_eq!(summary.sok_mismatches, 0);
    assert_eq!((summary.batches, summary.batches_failed), (2, 0));
    // Only the first Mesa block's parent, the genesis, is known here.
    assert_eq!(
        summary
            .without_parent
            .iter()
            .map(|(h, _)| *h)
            .collect::<Vec<_>>(),
        [548215, 550855, 551056]
    );
    assert!(summary.unreadable.is_empty());
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert!(summary.ok());
}

#[test]
fn tampered_blocks_fail() {
    let data = read_block_json(&fixtures().join(format!("{TWO_PROOFS}.json.zst"))).unwrap();
    let header = mina_web_verifier::header_from_precomputed_value(&data).unwrap();
    let works = completed_works(&data).unwrap();
    let (height, hash) = (550855, &TWO_PROOFS[7..]);

    let checked = check(&header, &works, keys(), true, Checks::ALL).unwrap();
    assert_eq!(checked.state_hash, hash);
    assert!(checked.failures(height, hash).is_empty());
    assert_eq!(checked.batch, Some(true));
    assert_eq!(checked.proofs.len(), 2);

    // Another timestamp: another state hash, and the proof is for the old one.
    let mut tampered = header.clone();
    let time = &mut tampered.protocol_state.body.blockchain_state.timestamp.0 .0;
    *time = UInt64::from(time.as_u64() + 1);
    let checked = check(&tampered, &works, keys(), true, Checks::ALL).unwrap();
    let failures = checked.failures(height, hash);
    assert_eq!(checked.block_proof, Some(false));
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(failures[0].contains("not to the file name's hash"));
    assert!(failures[1].contains("block proof does not verify"));

    // Another fee: the proofs still verify, but the sok digests are not those
    // of the work's fee and prover.
    let mut tampered = works.clone();
    let fee = &mut tampered[0].fee.0;
    *fee = UnsignedExtendedUInt64Int64ForVersionTagsStableV1(UInt64::from(fee.0.as_u64() + 1));
    let checked = check(&header, &tampered, keys(), true, Checks::ALL).unwrap();
    let failures = checked.failures(height, hash);
    assert_eq!(checked.block_proof, Some(true));
    assert!(checked.proofs.iter().all(|p| p.verified && !p.sok_matches));
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(failures.iter().all(|f| f.contains("sok digest")));

    // The work's two proofs with each other's statements: the batch fails,
    // then each proof on its own.
    let mut tampered = works.clone();
    let TransactionSnarkWorkTStableV2Proofs::Two((p, q)) = &mut tampered[0].proofs else {
        panic!("550855's work has two proofs");
    };
    std::mem::swap(&mut p.0.statement, &mut q.0.statement);
    let checked = check(&header, &tampered, keys(), true, Checks::ALL).unwrap();
    let failures = checked.failures(height, hash);
    assert_eq!(checked.batch, Some(false));
    assert!(checked.proofs.iter().all(|p| !p.verified && p.sok_matches));
    assert_eq!(
        failures,
        [
            format!(
                "work 0.0 ({}): transaction proof does not verify",
                checked.proofs[0].kind
            ),
            format!(
                "work 0.1 ({}): transaction proof does not verify",
                checked.proofs[1].kind
            ),
        ]
    );
}

#[test]
fn unreadable_and_other_chain_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join(format!("{ONE_PROOF}.json"));
    std::fs::write(&broken, b"{\"data\": {\"protocol_state\": 1}}").unwrap();
    let files = list_blocks(dir.path()).unwrap();
    let results = check_files(&files, keys(), &options(), |_| {});
    assert!(matches!(results[0].outcome, Outcome::Unreadable(_)));
    let summary = Summary::new(MAINNET_MESA_GENESIS, &results);
    assert!(!summary.ok());

    // Against the Berkeley genesis, the Mesa blocks are the other chain.
    let file = BlockFile::from_path(&fixtures().join(format!("{TWO_PROOFS}.json.zst"))).unwrap();
    let options = Options {
        genesis_state_hash: BERKELEY_GENESIS.to_string(),
        ..options()
    };
    let results = check_files(&[file], keys(), &options, |_| {});
    let Outcome::OtherChain {
        genesis_state_hash,
        protocol_version,
    } = &results[0].outcome
    else {
        panic!("{:?}", results[0].outcome);
    };
    assert_eq!(
        (genesis_state_hash.as_str(), protocol_version.as_str()),
        (MAINNET_MESA_GENESIS, "4.0.0")
    );
}

#[test]
fn tampered_headers_fail() {
    let data = read_block_json(&fixtures().join(format!("{ONE_PROOF}.json.zst"))).unwrap();
    let (height, hash) = (551056, &ONE_PROOF[7..]);
    let only_header = Options {
        checks: Checks {
            block: false,
            transactions: false,
            zkapps: false,
            header: true,
        },
        ..options()
    };
    let c = checked(check_json(data.clone(), keys(), &only_header).unwrap());
    assert_eq!((c.delta_chain, c.body_reference), (Some(true), Some(true)));
    assert_eq!(c.block_proof, None);
    assert!(c.failures(height, hash).is_empty());

    // A delta block chain proof from the grandparent with no body hashes.
    let mut tampered = data.clone();
    tampered["delta_transition_chain_proof"][0] =
        tampered["protocol_state"]["body"]["blockchain_state"]["genesis_ledger_hash"].clone();
    let c = checked(check_json(tampered, keys(), &only_header).unwrap());
    assert_eq!((c.delta_chain, c.body_reference), (Some(false), Some(true)));
    assert_eq!(
        c.failures(height, hash),
        ["delta block chain proof does not lead to the parent"]
    );

    // A body without its completed work no longer hashes to the header's
    // body reference.
    let mut tampered = data;
    let works = tampered["staged_ledger_diff"]["diff"][0]["completed_works"]
        .as_array_mut()
        .unwrap();
    assert!(works.pop().is_some());
    let c = checked(check_json(tampered, keys(), &only_header).unwrap());
    assert_eq!((c.delta_chain, c.body_reference), (Some(true), Some(false)));
    assert_eq!(
        c.failures(height, hash),
        ["the body does not hash to the header's body reference"]
    );
}

#[test]
fn zkapp_proofs() {
    let data = read_block_json(&fixtures().join(format!("{ZKAPP}.json.zst"))).unwrap();
    let (height, hash) = (548215, ZKAPP[7..].to_string());
    let only_zkapps = Options {
        checks: Checks {
            block: false,
            transactions: false,
            zkapps: true,
            header: false,
        },
        ..options()
    };
    let result = |c: Box<mina_block_replay::verify::Checked>| BlockResult {
        height,
        hash: hash.clone(),
        outcome: Outcome::Checked(c),
    };
    let c = checked(check_json(data, keys(), &only_zkapps).unwrap());
    assert_eq!(c.zkapp_keys.len(), 1);
    assert_eq!(c.zkapp_proofs.len(), 1);
    assert_eq!(c.zkapp_proofs[0].label, "0.2");
    assert_eq!(c.zkapp_proofs[0].result, ZkappResult::Pending);

    let mut results = vec![result(c.clone())];
    verify_zkapp_proofs(&mut results, &Default::default());
    let c_ok = checked(results.remove(0).outcome);
    assert_eq!(c_ok.zkapp_proofs[0].result, ZkappResult::Verified);
    assert!(c_ok.failures(height, &hash).is_empty());

    // Another statement: the proof is for the original one.
    let mut tampered = c.clone();
    let statement = &mut tampered.zkapp_proofs[0].statement;
    statement.calls.0 += mina_curves::pasta::Fp::from(1u64);
    let mut results = vec![result(tampered)];
    verify_zkapp_proofs(&mut results, &Default::default());
    let failed = checked(results.remove(0).outcome);
    assert_eq!(failed.zkapp_proofs[0].result, ZkappResult::Failed);
    let failures = failed.failures(height, &hash);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("does not verify"));

    // Without the block that sets the key, there is no key to check against.
    let mut without_key = c;
    without_key.zkapp_keys.clear();
    let mut results = vec![result(without_key)];
    verify_zkapp_proofs(&mut results, &Default::default());
    let missing = checked(results.remove(0).outcome);
    assert_eq!(missing.zkapp_proofs[0].result, ZkappResult::NoKey);
    assert!(missing.failures(height, &hash)[0].contains("has no key"));

    // A ledger whose account holds the key serves the proof instead, as for
    // a key set before the run's first block.
    let key = c_ok.zkapp_keys[0].clone();
    let account = |key: VerificationKey| Account {
        zkapp: Some(Box::new(ZkAppAccount {
            verification_key: Some(VerificationKeyWire::new(key)),
            ..Default::default()
        })),
        ..Account::empty()
    };
    let ledger_keys = zkapp_keys_of_accounts(&[Account::empty(), account(key.clone())]);
    assert_eq!(ledger_keys.len(), 1);
    let mut results = vec![result(missing.clone())];
    verify_zkapp_proofs(&mut results, &ledger_keys);
    let from_ledger = checked(results.remove(0).outcome);
    assert_eq!(from_ledger.zkapp_proofs[0].result, ZkappResult::Verified);
    assert!(from_ledger.zkapp_proofs[0].key_from_ledger);
    assert!(from_ledger.failures(height, &hash).is_empty());
    let summary = Summary::new(MAINNET_MESA_GENESIS, &[result(from_ledger)]);
    assert_eq!(
        (
            summary.zkapp_proofs_ledger_key,
            summary.zkapp_ledger_keys_used
        ),
        (1, 1)
    );

    // A key the block sets is used before a ledger's.
    let mut results = vec![result(c_ok.clone())];
    verify_zkapp_proofs(&mut results, &ledger_keys);
    let from_block = checked(results.remove(0).outcome);
    assert_eq!(from_block.zkapp_proofs[0].result, ZkappResult::Verified);
    assert!(!from_block.zkapp_proofs[0].key_from_ledger);

    // A ledger key is found by the hash computed from it, so another key
    // can't serve the proof, whatever hash the account stores with it.
    let mut other = key;
    other.actual_wrap_domain_size = match other.actual_wrap_domain_size {
        ProofVerified::N2 => ProofVerified::N1,
        _ => ProofVerified::N2,
    };
    let stored_hash = c_ok.zkapp_proofs[0].vk_hash;
    let lying = Account {
        zkapp: Some(Box::new(ZkAppAccount {
            verification_key: Some(VerificationKeyWire::with_hash(other, stored_hash)),
            ..Default::default()
        })),
        ..Account::empty()
    };
    let wrong_keys = zkapp_keys_of_accounts(&[lying]);
    assert!(!wrong_keys.contains_key(&stored_hash));
    let mut results = vec![result(missing)];
    verify_zkapp_proofs(&mut results, &wrong_keys);
    let still_missing = checked(results.remove(0).outcome);
    assert_eq!(still_missing.zkapp_proofs[0].result, ZkappResult::NoKey);
    assert!(!still_missing.zkapp_proofs[0].key_from_ledger);
}
