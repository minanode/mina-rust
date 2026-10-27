//! `mina_block_replay::verify` on a Mesa devnet block, with devnet's keys, and
//! a zkApp key from a ledger:
//!
//! - 570120 `3NLWtuFr…`, from the public bucket as fetched: one applied zkApp
//!   command with an account update proved against the key of hash
//!   `0x1972ADF1…`, which the block does not set;
//! - that key, as OCaml wrote it where block 569564 `3NKPLuuH…` sets it: the
//!   `verification_key` of its account update, `{ "data", "hash" }`.
//!
//! The block proof must verify against the embedded devnet blockchain key and
//! not against the live mainnet one (`data/vk`). The header checks must pass.
//! The zkApp proof has no key until an account of a ledger holds the key, as
//! accounts hold the keys set before a run's first block; then it verifies.
//!
//! The network config is process-wide, so devnet blocks are tested here, apart
//! from the mainnet ones in `verify_every_block.rs`. About 5 s with
//! `--release`:
//!
//! ```text
//! cargo test --release -p mina-block-replay --test verify_devnet_blocks
//! ```

use std::path::{Path, PathBuf};

use ledger::{
    proofs::verifiers::{BlockVerifier, TransactionVerifier},
    Account, VerificationKey, VerificationKeyWire, ZkAppAccount,
};
use mina_block_replay::{
    read_block_json,
    verify::{
        check_json, hex, verify_zkapp_proofs, zkapp_keys_of_accounts, BlockResult, Checked, Checks,
        Keys, Options, Outcome, Summary, ZkappResult, DEVNET_MESA_GENESIS,
    },
};
use mina_p2p_messages::v2::MinaBaseVerificationKeyWireStableV1;
use serde::Deserialize;

const BLOCK: &str = "570120-3NLWtuFrYuet6NvY4TxBWbHCuEm7L6oHkVU2auGGqVzoDakR18Er";
const KEY: &str = include_str!("fixtures/devnet-569564-verification-key.json");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn devnet_keys() -> Keys {
    let _ = mina_core::NetworkConfig::init("devnet");
    assert_eq!(mina_core::NetworkConfig::global().name, "devnet");
    mina_core::set_work_dir(std::env::temp_dir().join("verify-devnet-blocks-test"));
    Keys {
        block: BlockVerifier::make().into(),
        transaction: TransactionVerifier::make().into(),
        ledger_zkapp: Default::default(),
    }
}

fn options() -> Options {
    Options {
        genesis_state_hash: DEVNET_MESA_GENESIS.to_string(),
        batch: true,
        checks: Checks::ALL,
    }
}

fn checked(outcome: Outcome) -> Box<Checked> {
    match outcome {
        Outcome::Checked(c) => c,
        other => panic!("{other:?}"),
    }
}

/// The key 569564 sets, and the hash OCaml wrote next to it.
fn key_569564() -> (VerificationKey, String) {
    #[derive(Deserialize)]
    struct WithHash {
        hash: String,
    }
    let wire: MinaBaseVerificationKeyWireStableV1 = serde_json::from_str(KEY).unwrap();
    let WithHash { hash } = serde_json::from_str(KEY).unwrap();
    ((&wire).try_into().unwrap(), hash)
}

fn account_with(key: VerificationKey) -> Account {
    Account {
        zkapp: Some(Box::new(ZkAppAccount {
            verification_key: Some(VerificationKeyWire::new(key)),
            ..Default::default()
        })),
        ..Account::empty()
    }
}

#[test]
fn devnet_block_with_a_ledger_key() {
    let keys = devnet_keys();
    let (height, hash) = (570120, BLOCK[7..].to_string());
    let data = read_block_json(&fixtures().join(format!("devnet-{BLOCK}.json.zst"))).unwrap();
    let c = checked(check_json(data.clone(), &keys, &options()).unwrap());
    assert_eq!(c.state_hash, hash);
    assert_eq!(c.protocol_version, "4.0.0");
    assert_eq!(c.block_proof, Some(true));
    assert_eq!((c.delta_chain, c.body_reference), (Some(true), Some(true)));
    assert_eq!((c.works, c.zkapp_keys.len()), (0, 0));
    assert_eq!(c.zkapp_proofs.len(), 1);
    let named = hex(c.zkapp_proofs[0].vk_hash);

    let result = |c: Box<Checked>| BlockResult {
        height,
        hash: hash.clone(),
        outcome: Outcome::Checked(c),
    };

    // The block doesn't set the key its proof names, so without a ledger
    // there is none.
    let mut results = vec![result(c.clone())];
    verify_zkapp_proofs(&mut results, &keys.ledger_zkapp);
    let missing = checked(results.remove(0).outcome);
    assert_eq!(missing.zkapp_proofs[0].result, ZkappResult::NoKey);
    let failures = missing.failures(height, &hash);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("has no key"), "{failures:?}");

    // The key 569564 sets hashes to what OCaml wrote, which is the hash the
    // proof names; held by an account, it serves the proof.
    let (key, ocaml_hash) = key_569564();
    assert_eq!(hex(key.hash()), ocaml_hash.to_lowercase());
    assert_eq!(named, ocaml_hash.to_lowercase());
    let ledger_keys = zkapp_keys_of_accounts(&[Account::empty(), account_with(key)]);
    let mut results = vec![result(c)];
    verify_zkapp_proofs(&mut results, &ledger_keys);
    let summary = Summary::new(DEVNET_MESA_GENESIS, &results);
    let verified = checked(results.remove(0).outcome);
    assert_eq!(verified.zkapp_proofs[0].result, ZkappResult::Verified);
    assert!(verified.zkapp_proofs[0].key_from_ledger);
    assert!(verified.failures(height, &hash).is_empty());
    assert!(summary.ok(), "{summary}");
    assert_eq!(
        (
            summary.zkapp_proofs_verified,
            summary.zkapp_proofs_ledger_key
        ),
        (1, 1)
    );

    // The live mainnet blockchain key rejects the devnet block proof.
    let mainnet_key = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vk/mainnet-mesa-blockchain-vk.json"),
    )
    .unwrap();
    let mainnet = Keys {
        block: mina_web_verifier::verifier_index_auto(&mainnet_key)
            .unwrap()
            .into(),
        ..keys
    };
    let only_block = Options {
        checks: Checks {
            block: true,
            transactions: false,
            zkapps: false,
            header: false,
        },
        ..options()
    };
    let rejected = checked(check_json(data, &mainnet, &only_block).unwrap());
    assert_eq!(rejected.block_proof, Some(false));
    assert_eq!(
        rejected.failures(height, &hash),
        ["block proof does not verify"]
    );
}
