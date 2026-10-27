//! Blocks around the Mesa forks against the Mesa blockchain keys.
//!
//! The public `mina_network_block_data` bucket holds blocks of both the old
//! chain and the Mesa chain at the heights just after each Mesa fork: on devnet
//! the Berkeley chain kept extending the fork block 3NLYmfj4… up to height
//! 545493. A block must verify exactly when it belongs to the Mesa chain (its
//! `genesis_state_hash` is the Mesa genesis).
//!
//! Devnet blocks are checked three ways, and each must agree:
//!
//! - the OCaml key `data/vk/devnet-mesa-blockchain-vk.json`;
//! - the embedded `devnet_blockchain_verifier_index.json`, exported from that
//!   key with `export_verifier_index`, loaded by mina-web-verifier;
//! - mina-tree's own `BlockVerifier` for devnet, which loads the same embedded
//!   index through `make_verifier_index` and its cache.
//!
//! `embedded_devnet_index_verifies_only_mesa_fixtures` runs by default on the
//! committed fixtures. `mesa_keys_verify_exactly_the_mesa_blocks` is ignored by
//! default because it needs a directory of bucket blocks:
//!
//! ```text
//! R13_FORK_BLOCKS=<dir of <network>-<height>-<state hash>.json> \
//!   cargo test --release -p mina-web-verifier --test fork_boundary_blocks \
//!   -- --ignored --nocapture
//! ```
//!
//! `BlockVerifier` caches its finalized index under
//! `$HOME/.mina/circuit-blobs/<circuits directory>/`, overwriting any cache
//! built from another index, and failed verifications are dumped under
//! `$TMPDIR/mina-web-verifier`. On a shared machine, run the test binary with
//! `HOME` and `TMPDIR` set to scratch directories.

use ledger::{
    proofs::{verification::verify_block, verifiers::BlockVerifier},
    verifier::get_srs,
};
use mina_curves::pasta::Fp;
use mina_web_verifier::{header_from_precomputed, Error, Verifier};

const DEVNET_VK: &str = include_str!("../../../data/vk/devnet-mesa-blockchain-vk.json");
const MAINNET_VK: &str = include_str!("../../../data/vk/mainnet-mesa-blockchain-vk.json");
const DEVNET_EMBEDDED_INDEX: &str =
    include_str!("../../ledger/src/proofs/data/devnet_blockchain_verifier_index.json");

/// `genesis_state_hash` of every Mesa devnet block.
const DEVNET_MESA_GENESIS: &str = "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ";
/// `genesis_state_hash` of every Mesa mainnet block.
const MAINNET_MESA_GENESIS: &str = "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7";

/// Two Mesa devnet blocks and a Berkeley one at the height of the Mesa genesis.
const DEVNET_FIXTURES: [(&str, &str, bool); 3] = [
    (
        "devnet-545434 (Berkeley)",
        include_str!("fixtures/devnet-545434.json"),
        false,
    ),
    (
        "devnet-545435 (Mesa)",
        include_str!("fixtures/devnet-545435.json"),
        true,
    ),
    (
        "devnet-570000 (Mesa)",
        include_str!("fixtures/devnet-570000.json"),
        true,
    ),
];

fn verdict(result: Result<impl Sized, Error>) -> &'static str {
    match result {
        Ok(_) => "verifies",
        Err(Error::ProofInvalid) => "fails",
        Err(_) => "error",
    }
}

/// The devnet verifiers: the OCaml key, the embedded index through
/// mina-web-verifier, and `BlockVerifier`.
struct Devnet {
    ocaml: Verifier,
    embedded: Verifier,
    block_verifier: BlockVerifier,
}

impl Devnet {
    fn new() -> Self {
        // The embedded index `BlockVerifier` picks depends on the global network.
        let _ = mina_core::NetworkConfig::init("devnet");
        assert_eq!(mina_core::NetworkConfig::global().name, "devnet");
        Self {
            ocaml: Verifier::from_vk_json(DEVNET_VK).expect("devnet key"),
            embedded: Verifier::from_vk_json(DEVNET_EMBEDDED_INDEX).expect("embedded devnet index"),
            block_verifier: BlockVerifier::make(),
        }
    }

    /// The three verdicts on a precomputed block JSON.
    fn verdicts(&self, json: &str) -> [&'static str; 3] {
        let header = header_from_precomputed(json).expect("decode block");
        let block_verifier = if verify_block(&header, &self.block_verifier, &get_srs::<Fp>()) {
            "verifies"
        } else {
            "fails"
        };
        [
            verdict(self.ocaml.verify_precomputed(json)),
            verdict(self.embedded.verify_precomputed(json)),
            block_verifier,
        ]
    }
}

#[test]
fn embedded_devnet_index_verifies_only_mesa_fixtures() {
    let devnet = Devnet::new();
    for (name, json, mesa) in DEVNET_FIXTURES {
        let expected = if mesa { "verifies" } else { "fails" };
        assert_eq!(
            devnet.verdicts(json),
            [expected; 3],
            "{name}: devnet key, embedded index, BlockVerifier"
        );
    }
}

#[test]
#[ignore = "needs block files in R13_FORK_BLOCKS (R13, P1b)"]
fn mesa_keys_verify_exactly_the_mesa_blocks() {
    let dir = std::env::var("R13_FORK_BLOCKS").expect("R13_FORK_BLOCKS");
    let devnet = Devnet::new();
    let mainnet = Verifier::from_vk_json(MAINNET_VK).expect("mainnet key");

    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("read R13_FORK_BLOCKS")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no blocks in {dir}");

    let mut wrong = Vec::new();
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let json = std::fs::read_to_string(&path).expect("read block");
        let value: serde_json::Value = serde_json::from_str(&json).expect("block JSON");
        let data = value.get("data").unwrap_or(&value);
        let genesis = data["protocol_state"]["body"]["genesis_state_hash"]
            .as_str()
            .expect("genesis_state_hash")
            .to_owned();
        let version = &data["protocol_version"];
        let version = format!(
            "{}.{}.{}",
            version["transaction"], version["network"], version["patch"]
        );

        let (mesa, verdicts, results) = if name.starts_with("devnet-") {
            let [ocaml, embedded, block_verifier] = devnet.verdicts(&json);
            (
                genesis == DEVNET_MESA_GENESIS,
                vec![ocaml, embedded, block_verifier],
                vec![
                    format!("devnet key {ocaml}"),
                    format!("embedded index {embedded}"),
                    format!("BlockVerifier {block_verifier}"),
                ],
            )
        } else {
            let ocaml = verdict(mainnet.verify_precomputed(&json));
            (
                genesis == MAINNET_MESA_GENESIS,
                vec![ocaml],
                vec![format!("mainnet key {ocaml}")],
            )
        };
        let chain = if mesa { "Mesa" } else { "pre-Mesa" };
        println!(
            "{name}: {chain} (protocol {version}, genesis {}…): {}",
            &genesis[..12],
            results.join(", ")
        );
        let expected = if mesa { "verifies" } else { "fails" };
        if verdicts.iter().any(|result| *result != expected) {
            wrong.push(name);
        }
    }
    assert!(
        wrong.is_empty(),
        "Mesa keys should verify exactly the Mesa blocks: {wrong:?}"
    );
}
