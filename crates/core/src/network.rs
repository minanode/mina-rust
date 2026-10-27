use once_cell::sync::OnceCell;
use poseidon::hash::{
    legacy,
    params::{CODA_SIGNATURE, MAINNET_ZKAPP_BODY, MINA_SIGNATURE_MAINNET, TESTNET_ZKAPP_BODY},
};

use crate::constants::ConstraintConstants;

// From mina-signer, to avoid dependency
#[derive(Debug, Clone)]
pub enum NetworkId {
    /// Id for all testnets
    TESTNET = 0x00,

    /// Id for mainnet
    MAINNET = 0x01,
}

#[derive(Debug)]
pub struct NetworkConfig {
    pub name: &'static str,
    pub network_id: NetworkId,
    pub signature_prefix: &'static poseidon::hash::LazyParam,
    pub legacy_signature_prefix: &'static poseidon::hash::LazyParam,
    pub account_update_hash_param: &'static poseidon::hash::LazyParam,
    pub constraint_system_digests: &'static [[u8; 16]],
    pub default_peers: Vec<&'static str>,
    pub circuits_config: &'static CircuitsConfig,
    pub constraint_constants: &'static ConstraintConstants,
}

#[derive(Debug)]
pub struct CircuitsConfig {
    pub directory_name: &'static str,

    pub step_transaction_gates: &'static str,
    pub wrap_transaction_gates: &'static str,
    pub step_merge_gates: &'static str,
    pub step_blockchain_gates: &'static str,
    pub wrap_blockchain_gates: &'static str,
    pub step_transaction_opt_signed_opt_signed_gates: &'static str,
    pub step_transaction_opt_signed_gates: &'static str,
    pub step_transaction_proved_gates: &'static str,
}

static CONFIG: OnceCell<NetworkConfig> = OnceCell::new();
static RUNTIME_DIGESTS: OnceCell<[[u8; 16]; 6]> = OnceCell::new();

/// The six constraint-system digests the chain id hashes: the network's, or,
/// for a runtime fork whose blockchain circuits differ from the release's,
/// those with its blockchain-step digest.
pub fn constraint_system_digests() -> &'static [[u8; 16]] {
    RUNTIME_DIGESTS
        .get()
        .map(|digests| &digests[..])
        .unwrap_or(NetworkConfig::global().constraint_system_digests)
}

/// Replaces the blockchain-step digest, the last of the six, for a runtime
/// fork. Once per process, before the chain id is computed.
pub fn set_blockchain_step_digest(digest: [u8; 16]) -> Result<(), String> {
    let network = NetworkConfig::global().constraint_system_digests;
    let mut digests: [[u8; 16]; 6] = network
        .try_into()
        .map_err(|_| format!("expected 6 digests, the network has {}", network.len()))?;
    digests[5] = digest;
    if RUNTIME_DIGESTS.get_or_init(|| digests) != &digests {
        return Err("conflicting blockchain-step digests: restart for a different fork".into());
    }
    Ok(())
}

impl NetworkConfig {
    pub fn global() -> &'static Self {
        CONFIG.get_or_init(|| {
            let config = Self::default_config();
            crate::warn!(
                crate::log::system_time();
                kind = "network",
                message = "no network config initialized, using default config",
                config = config.name,
            );
            config
        })
    }

    pub fn init(network_name: &str) -> Result<(), String> {
        let config = match network_name {
            "devnet" => Self::devnet_config(),
            "mainnet" => Self::mainnet_config(),
            other => Err(format!("Unknown network {other}"))?,
        };

        CONFIG
            .set(config)
            .map_err(|_| "Double network configuration initialization".to_owned())?;

        Ok(())
    }

    fn default_config() -> Self {
        Self::devnet_config()
    }

    fn mainnet_config() -> Self {
        Self {
            name: mainnet::NAME,
            network_id: mainnet::NETWORK_ID,
            signature_prefix: &MINA_SIGNATURE_MAINNET,
            legacy_signature_prefix: &legacy::params::MINA_SIGNATURE_MAINNET,
            account_update_hash_param: &MAINNET_ZKAPP_BODY,
            constraint_system_digests: &mainnet::CONSTRAINT_SYSTEM_DIGESTS,
            default_peers: mainnet::default_peers(),
            circuits_config: &mainnet::CIRCUITS_CONFIG,
            constraint_constants: &mainnet::CONSTRAINT_CONSTANTS,
        }
    }

    fn devnet_config() -> Self {
        Self {
            name: devnet::NAME,
            network_id: devnet::NETWORK_ID,
            signature_prefix: &CODA_SIGNATURE,
            legacy_signature_prefix: &legacy::params::CODA_SIGNATURE,
            account_update_hash_param: &TESTNET_ZKAPP_BODY,
            constraint_system_digests: &devnet::CONSTRAINT_SYSTEM_DIGESTS,
            default_peers: devnet::default_peers(),
            circuits_config: &devnet::CIRCUITS_CONFIG,
            constraint_constants: &devnet::CONSTRAINT_CONSTANTS,
        }
    }
}

// Network constants

pub mod devnet {
    use super::{CircuitsConfig, NetworkId};
    use crate::constants::{ConstraintConstants, ForkConstants};

    pub const NETWORK_ID: NetworkId = NetworkId::TESTNET;
    pub const NAME: &str = "devnet";
    pub const SIGNATURE_PREFIX: &str = "CodaSignature";
    pub const ACCOUNT_UPDATE_HASH_PARAM: &str = "TestnetZkappBody";

    /// Mesa (4.0.x) digests, in the order the chain id hashes them. They
    /// depend on the signature kind, not the profile: devnet signs with the
    /// testnet kind, so transaction-base and the three zkApp digests differ
    /// from mainnet's, and transaction-merge is the same. The first five come
    /// from `mina advanced constraint-system-digests` with the 4.0.2 devnet
    /// profile. That command compiles the blockchain step circuit without the
    /// fork constants (it prints 9ea67872…), so the blockchain-step digest is
    /// read from the chain id input of `mina internal chain-id` with the Mesa
    /// devnet config instead (`scripts/ocaml-chain-id-trace.sh devnet`).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_proof/genesis_proof.ml#L146-L157>
    pub const CONSTRAINT_SYSTEM_DIGESTS: [[u8; 16]; 6] = [
        // transaction-merge
        [
            0xd7, 0x10, 0x89, 0xb3, 0xa1, 0x66, 0x95, 0x35, 0x99, 0x9e, 0x8f, 0x18, 0x1c, 0xd5,
            0x9a, 0xfc,
        ],
        // transaction-base
        [
            0x3b, 0xf6, 0xbb, 0x8a, 0x97, 0x66, 0x5f, 0xe7, 0xa9, 0xdf, 0x6f, 0xc1, 0x46, 0xe4,
            0xf9, 0x42,
        ],
        // zkapp-opt_signed-opt_signed
        [
            0x61, 0x4a, 0xec, 0x09, 0xed, 0x5e, 0x40, 0x68, 0xf4, 0x6d, 0x01, 0x0f, 0x00, 0x70,
            0x22, 0x6b,
        ],
        // zkapp-opt_signed
        [
            0x0f, 0xe3, 0x38, 0x1f, 0x50, 0x1f, 0x43, 0x27, 0x44, 0x72, 0x7c, 0x29, 0x6b, 0xe4,
            0x64, 0xb0,
        ],
        // zkapp-proved
        [
            0xca, 0xd5, 0x81, 0x43, 0x28, 0x31, 0xf1, 0x0f, 0xee, 0x99, 0x16, 0x15, 0x32, 0x50,
            0x49, 0x37,
        ],
        // blockchain-step
        [
            0xc3, 0x21, 0xd6, 0x49, 0xf1, 0x32, 0x29, 0x59, 0x17, 0x88, 0x9d, 0xdf, 0x75, 0x17,
            0x31, 0xd0,
        ],
    ];

    /// The devnet profile is the mainnet profile with another genesis
    /// timestamp and network name, so Mesa devnet has mainnet's constraint
    /// constants: 90 s slots and a 360 MINA coinbase (MIP-6). The devnet
    /// config overrides none of them. Only the fork point differs: it is
    /// `proof.fork` of the Mesa devnet config.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/devnet.ml#L1-L7>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/genesis_ledgers/devnet.json#L5-L11>
    pub const CONSTRAINT_CONSTANTS: ConstraintConstants = ConstraintConstants {
        sub_windows_per_window: 11,
        ledger_depth: 35,
        work_delay: 2,
        block_window_duration_ms: 90000,
        transaction_capacity_log_2: 7,
        pending_coinbase_depth: 5,
        coinbase_amount: 360000000000,
        supercharged_coinbase_factor: 1,
        account_creation_fee: 1000000000,
        // Fallback for legacy genesis bundles without runtime proof constants.
        // Runtime overrides also require matching circuit digests and keys;
        // see docs/hard-fork-runbook.md.
        // The Mesa hard fork point, block 3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib.
        // The global slot is the Mesa genesis slot, as in `proof.fork`; the
        // fork block itself is at 859399.
        fork: Some(ForkConstants {
            state_hash: ark_ff::MontFp!(
                "15173910414156258771540377045535399570012165554948415249653059754722861484487"
            ),
            blockchain_length: 545433,
            global_slot_since_genesis: 859560,
        }),
    };

    /// The Mesa (OCaml 4.0.2) devnet circuits, dumped with the devnet profile
    /// and the Mesa devnet fork config
    /// (`MINA_PROFILE=devnet tools/mesa-circuits/generate.sh <out>
    /// tools/mesa-circuits/mesa-devnet-fork.json`) into
    /// `<out>/circuit-blobs/4.0.2devnet`. They have the mainnet circuits'
    /// gates, wiring and sizes (`ROWS`, `AUX_LEN`); seven of the eight differ
    /// in a few constants: the signature prefix (`CodaSignature`, the testnet
    /// signature kind) in the transaction and zkApp step circuits, the fork
    /// constants in the blockchain step, and the step keys the two wrap
    /// circuits embed. The merge circuit is the same as mainnet's. The base
    /// circuit has the Berkeley devnet name, as mainnet's has its Berkeley
    /// name, but not its `_rows_rev.bin`, so the set has its own directory.
    pub const CIRCUITS_CONFIG: CircuitsConfig = CircuitsConfig {
        directory_name: "4.0.2devnet",

        step_transaction_gates: "step-step-proving-key-transaction-snark-transaction-0-c33ec5211c07928c87e850a63c6a2079",
        wrap_transaction_gates:
            "wrap-wrap-proving-key-transaction-snark-57d71923a9158dd01d40ca26805bc145",
        step_merge_gates:
            "step-step-proving-key-transaction-snark-merge-1-f970ffc5f94017c2d8b47516c890c21f",
        step_blockchain_gates:
            "step-step-proving-key-blockchain-snark-step-0-31e6069d62eefad69d12d2aa8c171c0c",
        wrap_blockchain_gates:
            "wrap-wrap-proving-key-blockchain-snark-a738bc9a1add41e0d0c1ba11fbe8b60d",
        step_transaction_opt_signed_opt_signed_gates: "step-step-proving-key-transaction-snark-opt_signed-opt_signed-2-0c43320f7e1e3070e95e698697745e97",
        step_transaction_opt_signed_gates:
            "step-step-proving-key-transaction-snark-opt_signed-3-961287d72c0f8a88a79cd6462bb88720",
        step_transaction_proved_gates:
            "step-step-proving-key-transaction-snark-proved-4-c7138b1ca988f74220f27805d2828365",
    };

    /// The seeds of `https://bootnodes.minaprotocol.com/networks/devnet.txt`,
    /// the `peer_list_url` of the Mesa devnet config. Seeds 2 and 3 listen on
    /// 32012 and 32013, not 10003.
    pub fn default_peers() -> Vec<&'static str> {
        vec![
            "/dns4/seed-1.devnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs",
            "/dns4/seed-2.devnet.gcp.o1test.net/tcp/32012/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag",
            "/dns4/seed-3.devnet.gcp.o1test.net/tcp/32013/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv",
        ]
    }
}

pub mod mainnet {
    use super::{CircuitsConfig, NetworkId};
    use crate::constants::{ConstraintConstants, ForkConstants};

    pub const NETWORK_ID: NetworkId = NetworkId::MAINNET;
    pub const NAME: &str = "mainnet";
    pub const SIGNATURE_PREFIX: &str = "MinaSignatureMainnet";
    pub const ACCOUNT_UPDATE_HASH_PARAM: &str = "MainnetZkappBody";

    /// Mesa (4.0.x) digests, in the order the chain id hashes them. The
    /// transaction and zkApp ones come from `mina advanced
    /// constraint-system-digests` with the 4.0.2 mainnet profile. That command
    /// compiles the blockchain step circuit without the fork constants, so
    /// its blockchain-step digest is read from the chain id input of `mina
    /// internal chain-id` with the Mesa mainnet config instead.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_proof/genesis_proof.ml#L146-L157>
    pub const CONSTRAINT_SYSTEM_DIGESTS: [[u8; 16]; 6] = [
        // transaction-merge
        [
            0xd7, 0x10, 0x89, 0xb3, 0xa1, 0x66, 0x95, 0x35, 0x99, 0x9e, 0x8f, 0x18, 0x1c, 0xd5,
            0x9a, 0xfc,
        ],
        // transaction-base
        [
            0xd3, 0x19, 0x48, 0xe6, 0x61, 0xcc, 0x66, 0x26, 0x75, 0xb0, 0xc0, 0x79, 0x45, 0x8f,
            0x71, 0x4a,
        ],
        // zkapp-opt_signed-opt_signed
        [
            0xdd, 0xaa, 0x38, 0x40, 0x5c, 0x20, 0xa8, 0xf7, 0xa7, 0xcf, 0x52, 0x35, 0xc1, 0xed,
            0x17, 0x13,
        ],
        // zkapp-opt_signed
        [
            0xd0, 0x48, 0x87, 0x7d, 0x85, 0xe3, 0x0a, 0x1f, 0xf9, 0xff, 0x4c, 0xbd, 0xfc, 0xc3,
            0x36, 0x39,
        ],
        // zkapp-proved
        [
            0x2d, 0x88, 0x10, 0xbd, 0xbd, 0xa3, 0x16, 0xe4, 0xb1, 0xf9, 0xf4, 0x1a, 0xe1, 0xb2,
            0x8f, 0x6c,
        ],
        // blockchain-step
        [
            0x55, 0x8d, 0x2b, 0x21, 0x1b, 0x60, 0x6c, 0xc0, 0x0d, 0xef, 0xff, 0x07, 0x5c, 0x1f,
            0x99, 0x13,
        ],
    ];

    /// Mesa halved the slot time (MIP-6) and the coinbase with it.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L1-L43>
    pub const CONSTRAINT_CONSTANTS: ConstraintConstants = ConstraintConstants {
        sub_windows_per_window: 11,
        ledger_depth: 35,
        work_delay: 2,
        block_window_duration_ms: 90000,
        transaction_capacity_log_2: 7,
        pending_coinbase_depth: 5,
        coinbase_amount: 360000000000,
        supercharged_coinbase_factor: 1,
        account_creation_fee: 1000000000,
        // Fallback for legacy genesis bundles without runtime proof constants.
        // Runtime overrides also require matching circuit digests and keys;
        // see docs/hard-fork-runbook.md.
        // The Mesa hard fork point, block 3NKHyxzgM9z4C1wUz5ZhJBmNhtDzDQGr1PGvWRwiCUcFvsdTco2T.
        fork: Some(ForkConstants {
            state_hash: ark_ff::MontFp!(
                "18327186670274677416352577188210688739283690409529737379307387200566072394530"
            ),
            blockchain_length: 548146,
            global_slot_since_genesis: 958440,
        }),
    };

    /// The Mesa (OCaml 4.0.2) mainnet circuits: their key-cache names, which
    /// are the md5 of each pickles step or wrap constraint system. No
    /// circuit-blobs release has them: `tools/mesa-circuits/generate.sh`
    /// dumps them with the OCaml binary into `<out>/circuit-blobs/4.0.2mainnet`,
    /// and `MINA_CIRCUIT_BLOBS_BASE_DIR=<out>/circuit-blobs` points the prover
    /// at them. The base circuit kept its Berkeley name but not its
    /// `_rows_rev.bin` (EndoMul gained a witness column), so Mesa blobs must not
    /// share a directory with the Berkeley `3.0.0mainnet` set.
    pub const CIRCUITS_CONFIG: CircuitsConfig = CircuitsConfig {
        directory_name: "4.0.2mainnet",

        step_transaction_gates: "step-step-proving-key-transaction-snark-transaction-0-b421ac835a0e73935f3d3569ff87f484",
        wrap_transaction_gates:
            "wrap-wrap-proving-key-transaction-snark-ae6b76daa712964c032f5261e18a72e1",
        step_merge_gates:
            "step-step-proving-key-transaction-snark-merge-1-f970ffc5f94017c2d8b47516c890c21f",
        step_blockchain_gates:
            "step-step-proving-key-blockchain-snark-step-0-12336df97ce8f76e9ed70f921cb69031",
        wrap_blockchain_gates:
            "wrap-wrap-proving-key-blockchain-snark-8b96fb94502d979e55c2e9750e08794e",
        step_transaction_opt_signed_opt_signed_gates: "step-step-proving-key-transaction-snark-opt_signed-opt_signed-2-f7e546d300f868e05268587acac717de",
        step_transaction_opt_signed_gates:
            "step-step-proving-key-transaction-snark-opt_signed-3-0a832b5be294b53b4f08c5c2d6aee4a9",
        step_transaction_proved_gates:
            "step-step-proving-key-transaction-snark-proved-4-bb6f6dfb983078a88eeaffe086460bb6",
    };

    pub fn default_peers() -> Vec<&'static str> {
        vec![
            "/dns4/mina-mainnet-seed.staketab.com/tcp/10003/p2p/12D3KooWSDTiXcdBVpN12ZqXJ49qCFp8zB1NnovuhZu6A28GLF1J",
            "/dns4/mina-seed.bitcat365.com/tcp/10001/p2p/12D3KooWQzozNTDKL7MqUh6Nh11GMA4pQhRCAsNTRWxCAzAi4VbE",
            "/dns4/production-mainnet-libp2p.minaprotocol.network/tcp/10000/p2p/12D3KooWPywsM191KGGNVGiNqN35nyyJg4W2BhhYukF6hP9YBR8q",
            "/dns4/seed-1.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWCa1d7G3SkRxy846qTvdAFX69NnoYZ32orWVLqJcDVGHW",
            "/dns4/seed-2.mainnet.gcp.o1test.net/tcp/32002/p2p/12D3KooWK4NfthViCTyLgVQa1WvqDC1NccVxGruCXCZUt3GqvFvn",
            "/dns4/seed-3.mainnet.gcp.o1test.net/tcp/32003/p2p/12D3KooWNofeYVAJXA3WGg2qCDhs3GEe71kTmKpFQXRbZmCz1Vr7",
            "/dns4/seed-4.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWEdBiTUQqxp3jeuWaZkwiSNcFxC6d6Tdq7u2Lf2ZD2Q6X",
            "/dns4/seed-5.mainnet.gcp.o1test.net/tcp/32005/p2p/12D3KooWL1DJTigSwuKQRfQE3p7puFUqfbHjXbZJ9YBWtMNpr3GU",
            "/dns4/seed-6.mainnet.gcp.o1test.net/tcp/32006/p2p/12D3KooWHGx4u32n42ub7dJNxAcAhwiA1WDq1Zsjn3k7RsS11pE8",
            "/dns4/seed.minaexplorer.com/tcp/8302/p2p/12D3KooWR7coZtrMHvsgsfiWq2GESYypac3i29LFGp6EpbtjxBiJ",
            "/dns4/seed.minataur.net/tcp/8302/p2p/12D3KooWNyExDzG8T1BYXHpXQS66kaw3zi6qi5Pg9KD3GEyHW5FF",
            "/dns4/seed.piconbello.com/tcp/10001/p2p/12D3KooWRFac2AztcTeen2DYNwnTrmVBvwNDsRiFpDVdTkwdFAHP",
        ]
    }
}
