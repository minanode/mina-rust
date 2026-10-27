//! Verify real Mesa blocks against the live Mesa verification keys in `data/vk`.
//!
//! Verification takes seconds in release builds and minutes in debug builds, so run
//! with `cargo test --release -p mina-web-verifier`.

use std::sync::OnceLock;

use mina_web_verifier::{header_from_precomputed, Error, ForkChoice, VerifiedBlock, Verifier};

const DEVNET_VK: &str = include_str!("../../../data/vk/devnet-mesa-blockchain-vk.json");
const MAINNET_VK: &str = include_str!("../../../data/vk/mainnet-mesa-blockchain-vk.json");

/// The Mesa genesis state hashes. Each Mesa genesis is the child of its network's
/// fork point, so it sits at the fork point's height + 1.
const DEVNET_MESA_GENESIS: &str = "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ";
const MAINNET_MESA_GENESIS: &str = "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7";
/// The Berkeley devnet genesis state hash, and the devnet fork point (`proof.fork`
/// in the Mesa devnet config, height 545433).
const DEVNET_BERKELEY_GENESIS: &str = "3NL93SipJfAMNDBRfQ8Uo8LPovC74mnJZfZYB5SK7mTtkL72dsPx";
const DEVNET_FORK_POINT: &str = "3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib";

const DEVNET_570000: &str = include_str!("fixtures/devnet-570000.json");
/// A Mesa devnet block at 545435, a child of the Mesa genesis: the one of the two
/// that the chain continued from.
const DEVNET_545435: &str = include_str!("fixtures/devnet-545435.json");
const DEVNET_545435_HASH: &str = "3NL1GQ579hqk3tsp16when7jKfjEw6NfA9Ud3ssnqPAwj3anXT3s";
/// A block of the old Berkeley chain (protocol 3.0.0), a child of the fork point.
/// That chain ran on for hours after the fork point, at the same heights as Mesa.
const DEVNET_545434_BERKELEY: &str = include_str!("fixtures/devnet-545434.json");
const DEVNET_545434_BERKELEY_HASH: &str = "3NKTXdtFDNoHmmY1Qmadkh5X5JdiTN8a5qZbdfGc4zSBydbR1cTn";
const MAINNET_560000: &str = include_str!("fixtures/mainnet-560000.json");
/// A Mesa mainnet block at 548148, a child of the Mesa genesis: the one of the
/// three that the chain continued from.
const MAINNET_548148: &str = include_str!("fixtures/mainnet-548148.json");
const MAINNET_548148_HASH: &str = "3NKtYZVHgyWrTzFE8j594Wy6HmFTb9rWw3sfPxZmZgRkrg572bAP";
/// Captured from live mainnet gossip on 2026-10-18.
const MAINNET_563978_GOSSIP: &[u8] = include_bytes!("fixtures/mainnet-563978.gossipbin");
/// Three competing blocks at one height; the chain continued from `3NL4y93Z…`.
const MAINNET_563933: [&str; 3] = [
    include_str!("fixtures/mainnet-563933-3NK7jUpc1UUNJQ.json"),
    include_str!("fixtures/mainnet-563933-3NKQvsbZaUZy86.json"),
    include_str!("fixtures/mainnet-563933-3NL4y93ZwB2ZCP.json"),
];
const MAINNET_563933_CANONICAL: &str = "3NL4y93ZwB2ZCPPo6THiJwJW2cTXnq4KnECNCWxTp71jHY6PApST";

fn devnet() -> &'static Verifier {
    static VERIFIER: OnceLock<Verifier> = OnceLock::new();
    VERIFIER.get_or_init(|| Verifier::from_vk_json(DEVNET_VK).expect("devnet key"))
}

fn mainnet() -> &'static Verifier {
    static VERIFIER: OnceLock<Verifier> = OnceLock::new();
    VERIFIER.get_or_init(|| Verifier::from_vk_json(MAINNET_VK).expect("mainnet key"))
}

#[test]
fn devnet_block_verifies() {
    let block = devnet()
        .verify_precomputed(DEVNET_570000)
        .expect("verifies");
    assert_eq!(block.facts.height, 570000);
    assert_eq!(
        block.facts.state_hash,
        "3NLQJUfnJdeSWiHEbVt2nLmqA9szuYRdvZQ6SuUdDKfQ1jSXSXG5"
    );
}

#[test]
fn mainnet_block_verifies() {
    let block = mainnet()
        .verify_precomputed(MAINNET_560000)
        .expect("verifies");
    assert_eq!(block.facts.height, 560000);
    assert_eq!(
        block.facts.state_hash,
        "3NKi82tB91KZWU1SWXJR6mab7DiTcbsCiCkCp45ptEd9Etr176Sb"
    );
}

#[test]
fn mainnet_gossip_block_verifies() {
    let block = mainnet()
        .verify_gossip(MAINNET_563978_GOSSIP)
        .expect("verifies");
    assert_eq!(block.facts.height, 563978);
}

#[test]
fn truncated_gossip_payload_is_rejected() {
    let truncated = &MAINNET_563978_GOSSIP[..MAINNET_563978_GOSSIP.len() - 1];
    assert!(matches!(
        mainnet().verify_gossip(truncated),
        Err(Error::InvalidBlock(_))
    ));
}

#[test]
fn block_is_rejected_by_the_other_network() {
    assert_eq!(
        mainnet().verify_precomputed(DEVNET_570000).err(),
        Some(Error::ProofInvalid)
    );
    assert_eq!(
        devnet().verify_precomputed(MAINNET_560000).err(),
        Some(Error::ProofInvalid)
    );
}

#[test]
fn tampered_block_is_rejected() {
    let mut block: serde_json::Value = serde_json::from_str(DEVNET_570000).unwrap();
    let length =
        &mut block["data"]["protocol_state"]["body"]["consensus_state"]["blockchain_length"];
    *length = serde_json::Value::String("570001".to_string());
    assert_eq!(
        devnet().verify_precomputed(&block.to_string()).err(),
        Some(Error::ProofInvalid)
    );
}

fn best_of(blocks: &[&VerifiedBlock]) -> String {
    let mut fork_choice = ForkChoice::default();
    for block in blocks {
        fork_choice.consider(block);
    }
    fork_choice.best().unwrap().facts.state_hash.clone()
}

#[test]
fn fork_choice_picks_the_block_the_chain_continued_from() {
    let competing: Vec<VerifiedBlock> = MAINNET_563933
        .iter()
        .map(|json| mainnet().verify_precomputed(json).expect("verifies"))
        .collect();
    assert!(competing.iter().all(|b| b.facts.height == 563933));

    let [a, b, c] = [&competing[0], &competing[1], &competing[2]];
    for order in [[a, b, c], [c, b, a], [b, c, a], [a, c, b]] {
        assert_eq!(best_of(&order), MAINNET_563933_CANONICAL);
    }
}

#[test]
fn fork_choice_prefers_the_longer_chain() {
    let older = mainnet().verify_precomputed(MAINNET_560000).unwrap();
    let newer = mainnet().verify_gossip(MAINNET_563978_GOSSIP).unwrap();
    assert_eq!(best_of(&[&newer, &older]), newer.facts.state_hash);
    assert_eq!(best_of(&[&older, &newer]), newer.facts.state_hash);
}

/// The first block on each Mesa chain, whose parent is the Mesa genesis.
#[test]
fn first_block_after_fork_verifies() {
    let block = devnet()
        .verify_precomputed(DEVNET_545435)
        .expect("verifies");
    assert_eq!(block.facts.height, 545435);
    assert_eq!(block.facts.state_hash, DEVNET_545435_HASH);
    assert_eq!(block.facts.genesis_state_hash, DEVNET_MESA_GENESIS);
    assert_eq!(block.facts.previous_state_hash, DEVNET_MESA_GENESIS);

    let block = mainnet()
        .verify_precomputed(MAINNET_548148)
        .expect("verifies");
    assert_eq!(block.facts.height, 548148);
    assert_eq!(block.facts.state_hash, MAINNET_548148_HASH);
    assert_eq!(block.facts.genesis_state_hash, MAINNET_MESA_GENESIS);
    assert_eq!(block.facts.previous_state_hash, MAINNET_MESA_GENESIS);
}

/// Blocks of the old Berkeley chain carry Berkeley proofs (another key, and the
/// kimchi of proof-systems 0.3.0), so a Mesa key rejects them, even at heights
/// right after the fork point.
#[test]
fn berkeley_chain_block_is_rejected() {
    // The block decodes and hashes to the name it has in the bucket, and it
    // belongs to the other chain: Berkeley's genesis, built on the fork point.
    let header = header_from_precomputed(DEVNET_545434_BERKELEY).expect("decodes");
    let state = &header.protocol_state;
    assert_eq!(
        header.try_hash().expect("hashes").to_string(),
        DEVNET_545434_BERKELEY_HASH
    );
    assert_eq!(
        state.body.consensus_state.blockchain_length.as_u32(),
        545434
    );
    assert_eq!(
        state.body.genesis_state_hash.to_string(),
        DEVNET_BERKELEY_GENESIS
    );
    assert_eq!(state.previous_state_hash.to_string(), DEVNET_FORK_POINT);

    assert_eq!(
        devnet().verify_precomputed(DEVNET_545434_BERKELEY).err(),
        Some(Error::ProofInvalid)
    );
}
