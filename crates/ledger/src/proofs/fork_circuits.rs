//! A runtime hard fork's blockchain circuits.
//!
//! The blockchain step circuit compiles `proof.fork` in, so a fork other than
//! the release's has its own blockchain step and wrap circuits, its own
//! blockchain verification key and its own blockchain-step digest in the chain
//! id. OCaml compiles them at startup from the runtime config. We read what
//! OCaml writes for that config offline, without a daemon:
//!
//! - the circuits (`tools/mesa-circuits/generate.sh <out> <config>`), from
//!   which we derive the verifier index as OCaml derives the key;
//! - the chain id's six constraint-system digests
//!   (`scripts/ocaml-chain-id-trace.sh <profile> <config>`, saved as
//!   `chain-id-digests.txt` next to the circuits). The chain id hashes the
//!   digest of the statement circuit inside the Pickles step circuit, which is
//!   not among the blobs, so it cannot be derived from them.
//!
//! - `fork-bundle.json`, recording the exact fork/zero-currency genesis and
//!   SHA-256 of all 24 circuit files and that trace. It is release provenance
//!   for trusted OCaml assets, not authentication of an untrusted dump.
//!
//! The transaction circuits do not depend on the fork.

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

use ark_ff::PrimeField;
use kimchi::circuits::gate::{Circuit, CircuitGate};
use mina_core::{
    constants::{self, ForkConstants},
    network::CircuitsConfig,
    ChainId,
};
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::v2::{MinaBaseProtocolConstantsCheckedValueStableV1, StateHash};
use o1_utils::hasher::CryptoDigest;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    constants::WrapBlockProof, field::FieldWitness, provers::decode_gates_with_public_input,
    transaction::make_prover_index, verifiers::BlockVerifier,
};

/// The release asset with the chain id's digests, in the blob directory.
pub const CHAIN_ID_DIGESTS_FILE: &str = "chain-id-digests.txt";

/// A trusted release bundle records the exact OCaml compilation's fork and
/// zero-currency genesis identity, and hashes every circuit file and trace.
/// Generate it after dumping the circuits and recording the trace with
/// `tools/mesa-circuits/bind-fork-bundle.py`.
pub const FORK_BUNDLE_FILE: &str = "fork-bundle.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkCircuitBundle {
    pub version: u32,
    pub fork: Option<ForkConstants>,
    pub genesis_state_hash: String,
    pub chain_id: String,
    pub files: BTreeMap<String, String>,
}

fn trace_value<'a>(trace: &'a str, prefix: &str) -> Result<&'a str, String> {
    let mut values = trace
        .lines()
        .filter(|line| *line != "chain_id printed by mina matches")
        .filter_map(|line| line.strip_prefix(prefix));
    let value = values
        .next()
        .ok_or_else(|| format!("missing {prefix} in {CHAIN_ID_DIGESTS_FILE}"))?;
    if values.next().is_some() || value.split_whitespace().count() != 1 {
        return Err(format!(
            "invalid or duplicate {prefix} in {CHAIN_ID_DIGESTS_FILE}"
        ));
    }
    Ok(value)
}

/// Validate before building or installing any verifier. The trace's identity
/// must be the identity computed from the requested config, not just a set of
/// internally consistent digests from some other OCaml run.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/chain_id/chain_id.ml#L63-L70>
fn validate_bundle(
    dir: &Path,
    fork: &Option<ForkConstants>,
    genesis_hash: &StateHash,
    constants: &MinaBaseProtocolConstantsCheckedValueStableV1,
    circuits: &CircuitsConfig,
    step: &str,
    wrap: &str,
    trace: &str,
) -> Result<[[u8; 16]; 6], String> {
    let path = dir.join(FORK_BUNDLE_FILE);
    let bundle: ForkCircuitBundle =
        serde_json::from_reader(std::fs::File::open(&path).map_err(|e| {
            format!(
                "{}: {e}; runtime fork requires a bound circuit bundle",
                path.display()
            )
        })?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bundle.version != 1 || &bundle.fork != fork {
        return Err("circuit bundle does not match the configured proof.fork".into());
    }
    let digests = parse_chain_id_digests(trace)?;
    let chain_id = ChainId::compute(
        &digests,
        genesis_hash,
        constants,
        constants::PROTOCOL_TRANSACTION_VERSION,
        constants::PROTOCOL_NETWORK_VERSION,
        &constants::TX_POOL_MAX_SIZE.into(),
    )
    .to_hex();
    if bundle.genesis_state_hash != genesis_hash.to_string()
        || trace_value(trace, "genesis_state_hash (zero currencies) ")? != genesis_hash.to_string()
        || bundle.chain_id != chain_id
        || trace_value(trace, "chain_id ")? != chain_id
    {
        return Err("circuit bundle/OCaml trace identity does not match the configured genesis and chain ID".into());
    }
    let mut names = vec![CHAIN_ID_DIGESTS_FILE.to_owned()];
    for stem in [
        circuits.step_transaction_gates,
        circuits.wrap_transaction_gates,
        circuits.step_merge_gates,
        circuits.step_transaction_opt_signed_opt_signed_gates,
        circuits.step_transaction_opt_signed_gates,
        circuits.step_transaction_proved_gates,
        step,
        wrap,
    ] {
        for suffix in ["_gates.json", "_internal_vars.bin", "_rows_rev.bin"] {
            names.push(format!("{stem}{suffix}"));
        }
    }
    names.sort();
    if bundle.files.keys().ne(names.iter()) {
        return Err("circuit bundle must hash exactly the 24 circuit files and OCaml trace".into());
    }
    for name in names {
        let digest = if name == CHAIN_ID_DIGESTS_FILE {
            hex::encode(Sha256::digest(trace.as_bytes()))
        } else {
            let path = dir.join(&name);
            let mut file =
                std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut hasher = Sha256::new();
            let mut buf = [0; 65536];
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            hex::encode(hasher.finalize())
        };
        if bundle.files[&name] != digest {
            return Err(format!("circuit bundle SHA-256 mismatch for {name}"));
        }
    }
    Ok(digests)
}

/// Browsers cannot load or install native fork bundles. Refuse before making
/// the embedded verifier or chain ID for an unsupported fork.
pub fn ensure_web_fork_supported(
    runtime: &Option<ForkConstants>,
    release: &Option<ForkConstants>,
) -> Result<(), String> {
    if runtime != release {
        return Err(
            "web node does not support a non-release proof.fork; use a release built for this fork"
                .into(),
        );
    }
    Ok(())
}

/// The chain id's digests in its order, as `ocaml-chain-id-split.py` names
/// them.
pub const CHAIN_ID_DIGEST_NAMES: [&str; 6] = [
    "transaction-merge",
    "transaction-base",
    "zkapp-opt_signed-opt_signed",
    "zkapp-opt_signed",
    "zkapp-proved",
    "blockchain-step",
];

/// What [`prepare_runtime_fork`] found and installed.
#[derive(Debug, Clone)]
pub struct RuntimeForkCircuits {
    /// The blob directory the circuits came from.
    pub dir: PathBuf,
    /// File prefixes of the fork's blockchain step and wrap circuits.
    pub step_blockchain_gates: String,
    pub wrap_blockchain_gates: String,
    /// The chain id's blockchain-step digest for the fork.
    pub blockchain_step_digest: [u8; 16],
}

static RUNTIME_FORK: OnceCell<RuntimeForkCircuits> = OnceCell::new();
static CIRCUITS_CONFIG: OnceCell<CircuitsConfig> = OnceCell::new();

/// OCaml's digest of a compiled circuit: MD5 of kimchi's circuit digest
/// (SHA-256 of the public input size and the gates). OCaml's key cache names
/// each circuit's files by it (`...-<digest>_gates.json`).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/kimchi_pasta_snarky_backend/plonk_constraint_system.ml#L1369-L1371>
pub fn constraint_system_digest<F: PrimeField>(
    public_input_size: usize,
    gates: &[CircuitGate<F>],
) -> [u8; 16] {
    md5::compute(Circuit::new(public_input_size, gates).digest()).0
}

/// The digest of a `*_gates.json` blob.
pub fn gates_file_digest<F: FieldWitness>(path: &Path) -> Result<[u8; 16], String> {
    let (public_input_size, gates) = read_gates::<F>(path)?;
    Ok(constraint_system_digest(public_input_size, &gates))
}

/// The digest a blob's file name claims: its last `-` component.
pub fn digest_in_name(stem: &str) -> Option<[u8; 16]> {
    let hex = stem.rsplit('-').next()?;
    let mut digest = [0; 16];
    hex::decode_to_slice(hex, &mut digest).ok()?;
    Some(digest)
}

fn read_gates<F: FieldWitness>(path: &Path) -> Result<(usize, Vec<CircuitGate<F>>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_gates_with_public_input(std::io::BufReader::with_capacity(1 << 20, file))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The gates of a blob, which must hash to the digest its name claims.
fn read_named_gates<F: FieldWitness>(
    dir: &Path,
    stem: &str,
) -> Result<Vec<CircuitGate<F>>, String> {
    let (public_input_size, gates) = read_gates::<F>(&dir.join(format!("{stem}_gates.json")))?;
    let digest = constraint_system_digest(public_input_size, &gates);
    if Some(digest) != digest_in_name(stem) {
        return Err(format!(
            "{stem}_gates.json hashes to {}, not the digest in its name",
            hex::encode(digest)
        ));
    }
    Ok(gates)
}

/// The one `<prefix>*_gates.json` in `dir`, without `_gates.json`.
fn find_one(dir: &Path, prefix: &str) -> Result<String, String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = entry
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if let Some(stem) = name.strip_suffix("_gates.json") {
            if stem.starts_with(prefix) {
                found.push(stem.to_owned());
            }
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no {prefix}*_gates.json in {}", dir.display())),
        _ => Err(format!(
            "{} {prefix}*_gates.json files in {}: keep one fork's circuits per directory",
            found.len(),
            dir.display()
        )),
    }
}

/// The six digests from `ocaml-chain-id-trace.sh`'s output: lines
/// `digest <name> <hex>`, in the chain id's order.
pub fn parse_chain_id_digests(text: &str) -> Result<[[u8; 16]; 6], String> {
    let mut digests = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("digest") {
            continue;
        }
        let (Some(name), Some(hex)) = (words.next(), words.next()) else {
            return Err(format!("bad digest line {line:?}"));
        };
        let mut digest = [0; 16];
        hex::decode_to_slice(hex, &mut digest).map_err(|e| format!("{name}: {e}"))?;
        digests.push((name.to_owned(), digest));
    }
    let names: Vec<&str> = digests.iter().map(|(n, _)| n.as_str()).collect();
    if names != CHAIN_ID_DIGEST_NAMES {
        return Err(format!(
            "expected the digests {CHAIN_ID_DIGEST_NAMES:?} in order, found {names:?}"
        ));
    }
    let mut result = [[0; 16]; 6];
    for (slot, (_, digest)) in result.iter_mut().zip(digests) {
        *slot = digest;
    }
    Ok(result)
}

/// The circuits the provers load: the network's, with the blockchain
/// circuits of the runtime fork if [`prepare_runtime_fork`] installed them.
pub fn circuits_config() -> &'static CircuitsConfig {
    CIRCUITS_CONFIG
        .get()
        .unwrap_or(mina_core::NetworkConfig::global().circuits_config)
}

pub fn runtime_fork_circuits() -> Option<&'static RuntimeForkCircuits> {
    RUNTIME_FORK.get()
}

/// Call once the runtime constants are initialized and before any block
/// verifier, prover or chain id is made. With the release's fork it does
/// nothing. With another fork it reads, from
/// `$MINA_CIRCUIT_BLOBS_BASE_DIR/<network circuits directory>/`, that fork's
/// blockchain circuits, [`CHAIN_ID_DIGESTS_FILE`] and [`FORK_BUNDLE_FILE`];
/// checks the configured genesis identity and every file hash; installs the verifier
/// index derived from the wrap circuit and the blockchain-step digest; and
/// points the block provers at the circuits. It refuses to run a different
/// fork without them: the embedded key and digest would make the node reject
/// every block of that chain and compute a chain id no peer has.
pub fn prepare_runtime_fork(
    genesis_hash: &StateHash,
    constants: &MinaBaseProtocolConstantsCheckedValueStableV1,
) -> Result<Option<&'static RuntimeForkCircuits>, String> {
    let network = mina_core::NetworkConfig::global();
    let runtime = &mina_core::constants::constraint_constants().fork;
    if runtime == &network.constraint_constants.fork {
        return Ok(None);
    }
    let base = std::env::var("MINA_CIRCUIT_BLOBS_BASE_DIR").map_err(|_| {
        format!(
            "the runtime config's proof.fork is not the {} release's: set \
             MINA_CIRCUIT_BLOBS_BASE_DIR to the circuits OCaml dumps for this config \
             (tools/mesa-circuits/generate.sh), with its {CHAIN_ID_DIGESTS_FILE} \
             (scripts/ocaml-chain-id-trace.sh) and {FORK_BUNDLE_FILE} \
             (tools/mesa-circuits/bind-fork-bundle.py)",
            network.name
        )
    })?;
    let dir = Path::new(&base).join(network.circuits_config.directory_name);
    let digests_path = dir.join(CHAIN_ID_DIGESTS_FILE);
    let trace = std::fs::read_to_string(&digests_path)
        .map_err(|e| format!("{}: {e}", digests_path.display()))?;
    let step = find_one(&dir, "step-step-proving-key-blockchain-snark-step-0-")?;
    let wrap = find_one(&dir, "wrap-wrap-proving-key-blockchain-snark-")?;
    let digests = validate_bundle(
        &dir,
        runtime,
        genesis_hash,
        constants,
        network.circuits_config,
        &step,
        &wrap,
        &trace,
    )?;
    if let Some(done) = RUNTIME_FORK.get() {
        return Ok(Some(done));
    }
    // The transaction circuits are the release's; only the blockchain step
    // depends on the fork.
    if digests[..5] != network.constraint_system_digests[..5] {
        return Err(format!(
            "{} has transaction digests of another release than {}",
            digests_path.display(),
            network.name
        ));
    }
    if digests[5] == network.constraint_system_digests[5] {
        return Err(format!(
            "{} has the {} release's own blockchain-step digest, not the runtime fork's",
            digests_path.display(),
            network.name
        ));
    }
    if digest_in_name(&step) == digest_in_name(network.circuits_config.step_blockchain_gates) {
        return Err(format!(
            "{step} is the {} release's blockchain step circuit, not the runtime fork's",
            network.name
        ));
    }
    read_named_gates::<Fp>(&dir, &step)?;
    let wrap_gates = read_named_gates::<Fq>(&dir, &wrap)?;
    let index = make_prover_index::<WrapBlockProof, Fq>(wrap_gates, None).verifier_index();
    BlockVerifier::install(index)?;
    mina_core::network::set_blockchain_step_digest(digests[5])?;

    let base_config = network.circuits_config;
    let leak = |s: &str| -> &'static str { Box::leak(s.to_owned().into_boxed_str()) };
    let _ = CIRCUITS_CONFIG.set(CircuitsConfig {
        directory_name: base_config.directory_name,
        step_transaction_gates: base_config.step_transaction_gates,
        wrap_transaction_gates: base_config.wrap_transaction_gates,
        step_merge_gates: base_config.step_merge_gates,
        step_blockchain_gates: leak(&step),
        wrap_blockchain_gates: leak(&wrap),
        step_transaction_opt_signed_opt_signed_gates: base_config
            .step_transaction_opt_signed_opt_signed_gates,
        step_transaction_opt_signed_gates: base_config.step_transaction_opt_signed_gates,
        step_transaction_proved_gates: base_config.step_transaction_proved_gates,
    });
    Ok(Some(RUNTIME_FORK.get_or_init(|| RuntimeForkCircuits {
        dir,
        step_blockchain_gates: step,
        wrap_blockchain_gates: wrap,
        blockchain_step_digest: digests[5],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_binds_fork_trace_and_every_circuit_file() {
        let dir = tempfile::tempdir().unwrap();
        let trace = include_str!("../../../../tests/files/fork-export/new/chain-id-digests.txt");
        let dump: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/files/fork-export/new/fork-circuits.json"
        ))
        .unwrap();
        let fork: Option<ForkConstants> =
            serde_json::from_value(dump["proof"]["fork"].clone()).unwrap();
        let genesis: StateHash = "3NLASv5h1h1iLVxhj3dZpRfxmSSYG6qPLk1oto1xuqPnm49L2VZ6"
            .parse()
            .unwrap();
        let mut constants = mina_core::constants::PROTOCOL_CONSTANTS.clone();
        // Expected identity and timestamp are OCaml's saved rehearsal trace.
        constants.genesis_state_timestamp =
            mina_p2p_messages::v2::BlockTimeTimeStableV1(1792635868000u64.into());
        let circuits = &mina_core::network::devnet::CIRCUITS_CONFIG;
        let step = "step-step-proving-key-blockchain-snark-step-0-91d2022a";
        let wrap = "wrap-wrap-proving-key-blockchain-snark-ce5413ae";
        let mut bundle = ForkCircuitBundle {
            version: 1,
            fork: fork.clone(),
            genesis_state_hash: genesis.to_string(),
            chain_id: "2cae926c47771e8e87a18d22f6ea78557d07ae9d43b68b35cbd4b18e5dc7176e".into(),
            files: BTreeMap::new(),
        };
        for stem in [
            circuits.step_transaction_gates,
            circuits.wrap_transaction_gates,
            circuits.step_merge_gates,
            circuits.step_transaction_opt_signed_opt_signed_gates,
            circuits.step_transaction_opt_signed_gates,
            circuits.step_transaction_proved_gates,
            step,
            wrap,
        ] {
            for suffix in ["_gates.json", "_internal_vars.bin", "_rows_rev.bin"] {
                let name = format!("{stem}{suffix}");
                std::fs::write(dir.path().join(&name), b"circuit fixture").unwrap();
                bundle
                    .files
                    .insert(name, hex::encode(Sha256::digest(b"circuit fixture")));
            }
        }
        bundle.files.insert(
            CHAIN_ID_DIGESTS_FILE.into(),
            hex::encode(Sha256::digest(trace.as_bytes())),
        );
        let save = |bundle: &ForkCircuitBundle| {
            std::fs::write(
                dir.path().join(FORK_BUNDLE_FILE),
                serde_json::to_vec(bundle).unwrap(),
            )
            .unwrap()
        };
        let validate = |fork: &Option<ForkConstants>, genesis: &StateHash, trace: &str| {
            validate_bundle(
                dir.path(),
                fork,
                genesis,
                &constants,
                circuits,
                step,
                wrap,
                trace,
            )
        };
        // Missing binding metadata must refuse even with otherwise valid digests.
        assert!(validate(&fork, &genesis, trace).is_err());
        save(&bundle);
        validate(&fork, &genesis, trace).unwrap();
        let original = fork.as_ref().unwrap();
        for changed in [
            ForkConstants {
                state_hash: Fp::from(1),
                ..original.clone()
            },
            ForkConstants {
                blockchain_length: original.blockchain_length + 1,
                ..original.clone()
            },
            ForkConstants {
                global_slot_since_genesis: original.global_slot_since_genesis + 1,
                ..original.clone()
            },
        ] {
            assert!(validate(&Some(changed), &genesis, trace).is_err());
        }
        assert!(validate(&fork, &StateHash::zero(), trace).is_err());
        assert!(validate(
            &fork,
            &genesis,
            &trace.replace(&genesis.to_string(), &StateHash::zero().to_string())
        )
        .is_err());
        assert!(validate(
            &fork,
            &genesis,
            &trace.replace(&bundle.chain_id, &"0".repeat(64))
        )
        .is_err());
        assert!(validate(&fork, &genesis, &trace.replace("8e1ca52b", "8e1ca52c")).is_err());
        let file = format!("{step}_internal_vars.bin");
        std::fs::write(dir.path().join(&file), b"different circuit").unwrap();
        assert!(validate(&fork, &genesis, trace)
            .unwrap_err()
            .contains("SHA-256"));
        std::fs::write(dir.path().join(&file), b"circuit fixture").unwrap();
        bundle.files.remove(&file);
        save(&bundle);
        assert!(validate(&fork, &genesis, trace).is_err());
    }

    #[test]
    fn web_refuses_non_release_fork_before_verifier_creation() {
        let release = mina_core::network::devnet::CONSTRAINT_CONSTANTS
            .fork
            .clone();
        ensure_web_fork_supported(&release, &release).unwrap();
        let mut other = release.clone();
        other.as_mut().unwrap().blockchain_length += 1;
        assert!(ensure_web_fork_supported(&other, &release)
            .unwrap_err()
            .contains("non-release"));
    }

    /// `ocaml-chain-id-trace.sh devnet` output for the Mesa devnet config
    /// (R13): the pinned devnet digests come from it.
    const TRACE: &str = "genesis_state_hash (zero currencies) \
        3NL4SkiA4eULVQWPGe4LVBn4Goixk3icTD2886iT4y23HRrTPrwN
digest transaction-merge            d71089b3a1669535999e8f181cd59afc
digest transaction-base             3bf6bb8a97665fe7a9df6fc146e4f942
digest zkapp-opt_signed-opt_signed  614aec09ed5e4068f46d010f0070226b
digest zkapp-opt_signed             0fe3381f501f432744727c296be464b0
digest zkapp-proved                 cad581432831f10fee99161532504937
digest blockchain-step              c321d649f132295917889ddf751731d0
";

    #[test]
    fn trace_digests_parse_in_order() {
        let digests = parse_chain_id_digests(TRACE).unwrap();
        assert_eq!(
            digests.as_slice(),
            mina_core::network::devnet::CONSTRAINT_SYSTEM_DIGESTS.as_slice()
        );
        let swapped = TRACE
            .replace("transaction-merge", "x")
            .replace("transaction-base", "transaction-merge");
        assert!(parse_chain_id_digests(&swapped).is_err());
    }

    #[test]
    fn digest_in_blob_names() {
        assert_eq!(
            digest_in_name(
                "step-step-proving-key-blockchain-snark-step-0-31e6069d62eefad69d12d2aa8c171c0c"
            )
            .map(hex::encode)
            .as_deref(),
            Some("31e6069d62eefad69d12d2aa8c171c0c")
        );
        assert_eq!(digest_in_name("no-digest"), None);
    }
}
