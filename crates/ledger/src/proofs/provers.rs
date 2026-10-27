use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result};
use kimchi::circuits::gate::CircuitGate;
use mina_core::network::CircuitsConfig;
use mina_curves::pasta::{Fp, Fq};
use once_cell::sync::OnceCell;

use super::{
    circuit_blobs,
    constants::{
        StepBlockProof, StepMergeProof, StepTransactionProof, StepZkappOptSignedOptSignedProof,
        StepZkappOptSignedProof, StepZkappProvedProof, WrapBlockProof, WrapTransactionProof,
    },
    field::FieldWitness,
    transaction::{make_prover_index, InternalVars, Prover, V},
    verifiers::{BlockVerifier, TransactionVerifier},
};

use mina_p2p_messages::binprot::{
    self,
    macros::{BinProtRead, BinProtWrite},
};

pub fn devnet_circuit_directory() -> &'static str {
    mina_core::network::devnet::CIRCUITS_CONFIG.directory_name
}

// TODO(tizoc): right now all tests are for devnets, and the above
// function is used for those tests.
// pub fn mainnet_circuit_directory() -> &'static str {
//     mina_core::network::devnet::CIRCUITS_CONFIG.directory_name
// }
//
// pub fn circuit_directory() -> &'static str {
//     mina_core::NetworkConfig::global().circuits_config.directory_name
// }

fn decode_gates_file<F: FieldWitness>(
    reader: impl std::io::Read,
) -> std::io::Result<Vec<CircuitGate<F>>> {
    decode_gates_with_public_input(reader).map(|(_, gates)| gates)
}

/// A `*_gates.json` blob: its public input size and gates.
pub(super) fn decode_gates_with_public_input<F: FieldWitness>(
    reader: impl std::io::Read,
) -> std::io::Result<(usize, Vec<CircuitGate<F>>)> {
    #[serde_with::serde_as]
    #[derive(serde::Deserialize)]
    struct GatesFile<F: ark_ff::PrimeField> {
        public_input_size: usize,
        #[serde_as(as = "Vec<_>")]
        gates: Vec<CircuitGate<F>>,
    }
    let data: GatesFile<F> = serde_json::from_reader(reader)?;
    Ok((data.public_input_size, data.gates))
}

fn read_gates_file<F: FieldWitness>(filename: &impl AsRef<Path>) -> Result<Vec<CircuitGate<F>>> {
    let bytes = circuit_blobs::fetch_blocking(filename)?;
    decode_gates_file(bytes.as_slice()).map_err(Into::into)
}

fn make_gates<F: FieldWitness>(
    filename: &str,
) -> Result<(
    HashMap<usize, (Vec<(F, V)>, Option<F>)>,
    Vec<Vec<Option<V>>>,
    Vec<CircuitGate<F>>,
)> {
    let circuits_config = mina_core::NetworkConfig::global().circuits_config;
    let base_dir = Path::new(&circuits_config.directory_name);

    let internal_vars_path = base_dir.join(format!("{}_internal_vars.bin", filename));
    let rows_rev_path = base_dir.join(format!("{}_rows_rev.bin", filename));
    let gates_path = base_dir.join(format!("{}_gates.json", filename));

    let gates: Vec<CircuitGate<F>> = read_gates_file(&gates_path)
        .with_context(|| format!("cannot load circuit file {}", gates_path.display()))?;
    let (internal_vars_path, rows_rev_path) =
        read_constraints_data::<F>(&internal_vars_path, &rows_rev_path)?;

    Ok((internal_vars_path, rows_rev_path, gates))
}

macro_rules! get_or_make {
    ($constant: ident, $type: ty, $filename: expr) => {{
        get_or_make!($constant, $type, None, $filename)
    }};
    ($constant: ident, $type: ty, $verifier_index: expr, $filename: expr) => {{
        if let Some(prover) = $constant.get() {
            return Ok(prover.clone());
        }

        let (internal_vars, rows_rev, gates) = make_gates($filename)?;

        let index = make_prover_index::<$type, _>(gates, $verifier_index);
        let prover = Prover {
            internal_vars,
            rows_rev,
            index,
        };

        Ok($constant.get_or_init(|| prover.into()).clone())
    }};
}

static TX_STEP_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();
static TX_WRAP_PROVER: OnceCell<Arc<Prover<Fq>>> = OnceCell::new();
static MERGE_STEP_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();
static BLOCK_STEP_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();
static BLOCK_WRAP_PROVER: OnceCell<Arc<Prover<Fq>>> = OnceCell::new();
static ZKAPP_STEP_OPT_SIGNED_OPT_SIGNED_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();
static ZKAPP_STEP_OPT_SIGNED_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();
static ZKAPP_STEP_PROOF_PROVER: OnceCell<Arc<Prover<Fp>>> = OnceCell::new();

fn default_circuits_config() -> &'static CircuitsConfig {
    super::fork_circuits::circuits_config()
}

mod prover_makers {
    use super::*;

    fn get_or_make_tx_step_prover(config: &CircuitsConfig) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(
            TX_STEP_PROVER,
            StepTransactionProof,
            config.step_transaction_gates
        )
    }
    fn get_or_make_tx_wrap_prover(
        config: &CircuitsConfig,
        verifier_index: Option<TransactionVerifier>,
    ) -> Result<Arc<Prover<Fq>>> {
        get_or_make!(
            TX_WRAP_PROVER,
            WrapTransactionProof,
            verifier_index.map(Into::into),
            config.wrap_transaction_gates
        )
    }
    fn get_or_make_merge_step_prover(config: &CircuitsConfig) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(MERGE_STEP_PROVER, StepMergeProof, config.step_merge_gates)
    }
    fn get_or_make_block_step_prover(config: &CircuitsConfig) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(
            BLOCK_STEP_PROVER,
            StepBlockProof,
            config.step_blockchain_gates
        )
    }
    fn get_or_make_block_wrap_prover(
        config: &CircuitsConfig,
        verifier_index: Option<BlockVerifier>,
    ) -> Result<Arc<Prover<Fq>>> {
        get_or_make!(
            BLOCK_WRAP_PROVER,
            WrapBlockProof,
            verifier_index.map(Into::into),
            config.wrap_blockchain_gates
        )
    }
    fn get_or_make_zkapp_step_opt_signed_opt_signed_prover(
        config: &CircuitsConfig,
    ) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(
            ZKAPP_STEP_OPT_SIGNED_OPT_SIGNED_PROVER,
            StepZkappOptSignedOptSignedProof,
            config.step_transaction_opt_signed_opt_signed_gates
        )
    }
    fn get_or_make_zkapp_step_opt_signed_prover(
        config: &CircuitsConfig,
    ) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(
            ZKAPP_STEP_OPT_SIGNED_PROVER,
            StepZkappOptSignedProof,
            config.step_transaction_opt_signed_gates
        )
    }
    fn get_or_make_zkapp_step_proof_prover(config: &CircuitsConfig) -> Result<Arc<Prover<Fp>>> {
        get_or_make!(
            ZKAPP_STEP_PROOF_PROVER,
            StepZkappProvedProof,
            config.step_transaction_proved_gates
        )
    }

    impl BlockProver {
        pub fn make(
            block_verifier_index: Option<BlockVerifier>,
            tx_verifier_index: Option<TransactionVerifier>,
        ) -> Result<Self> {
            let config = default_circuits_config();
            let block_step_prover = get_or_make_block_step_prover(config)?;
            let block_wrap_prover = get_or_make_block_wrap_prover(config, block_verifier_index)?;
            let tx_wrap_prover = get_or_make_tx_wrap_prover(config, tx_verifier_index)?;

            Ok(Self {
                block_step_prover,
                block_wrap_prover,
                tx_wrap_prover,
            })
        }
    }

    impl TransactionProver {
        pub fn make(tx_verifier_index: Option<TransactionVerifier>) -> Result<Self> {
            let config = default_circuits_config();
            let tx_step_prover = get_or_make_tx_step_prover(config)?;
            let tx_wrap_prover = get_or_make_tx_wrap_prover(config, tx_verifier_index)?;
            let merge_step_prover = get_or_make_merge_step_prover(config)?;

            Ok(Self {
                tx_step_prover,
                tx_wrap_prover,
                merge_step_prover,
            })
        }
    }

    impl ZkappProver {
        pub fn make(tx_verifier_index: Option<TransactionVerifier>) -> Result<Self> {
            let config = default_circuits_config();
            let tx_wrap_prover = get_or_make_tx_wrap_prover(config, tx_verifier_index)?;
            let merge_step_prover = get_or_make_merge_step_prover(config)?;
            let step_opt_signed_opt_signed_prover =
                get_or_make_zkapp_step_opt_signed_opt_signed_prover(config)?;
            let step_opt_signed_prover = get_or_make_zkapp_step_opt_signed_prover(config)?;
            let step_proof_prover = get_or_make_zkapp_step_proof_prover(config)?;

            Ok(Self {
                tx_wrap_prover,
                merge_step_prover,
                step_opt_signed_opt_signed_prover,
                step_opt_signed_prover,
                step_proof_prover,
            })
        }
    }
}

#[derive(Clone)]
pub struct BlockProver {
    pub block_step_prover: Arc<Prover<Fp>>,
    pub block_wrap_prover: Arc<Prover<Fq>>,
    pub tx_wrap_prover: Arc<Prover<Fq>>,
}

#[derive(Clone)]
pub struct TransactionProver {
    pub tx_step_prover: Arc<Prover<Fp>>,
    pub tx_wrap_prover: Arc<Prover<Fq>>,
    pub merge_step_prover: Arc<Prover<Fp>>,
}

#[derive(Clone)]
pub struct ZkappProver {
    pub tx_wrap_prover: Arc<Prover<Fq>>,
    pub merge_step_prover: Arc<Prover<Fp>>,
    pub step_opt_signed_opt_signed_prover: Arc<Prover<Fp>>,
    pub step_opt_signed_prover: Arc<Prover<Fp>>,
    pub step_proof_prover: Arc<Prover<Fp>>,
}

impl BlockProver {
    /// Called must make sure that before calling this,
    /// `BlockProver::make` call was made.
    pub fn get_once_made() -> Self {
        Self {
            block_step_prover: BLOCK_STEP_PROVER.wait().clone(),
            block_wrap_prover: BLOCK_WRAP_PROVER.wait().clone(),
            tx_wrap_prover: TX_WRAP_PROVER.wait().clone(),
        }
    }
}

fn decode_constraints_data<F: FieldWitness>(
    mut internal_vars: &[u8],
    mut rows_rev: &[u8],
    internal_vars_path: &Path,
    rows_rev_path: &Path,
) -> Result<(InternalVars<F>, Vec<Vec<Option<V>>>)> {
    use mina_p2p_messages::bigint::BigInt;

    impl From<&VRaw> for V {
        fn from(value: &VRaw) -> Self {
            match value {
                VRaw::External(index) => Self::External(*index as usize),
                VRaw::Internal(id) => Self::Internal(*id as usize),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite)]
    enum VRaw {
        External(u32),
        Internal(u32),
    }

    use binprot::BinProtRead;

    type InternalVarsRaw = HashMap<u32, (Vec<(BigInt, VRaw)>, Option<BigInt>)>;

    let internal_vars: InternalVarsRaw =
        HashMap::binprot_read(&mut internal_vars).with_context(|| {
            format!(
                "cannot decode circuit file {}",
                internal_vars_path.display()
            )
        })?;
    let rows_rev: Vec<Vec<Option<VRaw>>> = Vec::binprot_read(&mut rows_rev)
        .with_context(|| format!("cannot decode circuit file {}", rows_rev_path.display()))?;

    let internal_vars: InternalVars<F> = internal_vars
        .into_iter()
        .map(|(id, (list, opt))| {
            let id = id as usize;
            let list: Result<Vec<_>> = list
                .iter()
                .map(|(n, v)| Ok((n.to_field::<F>()?, V::from(v))))
                .collect();
            let opt = opt.as_ref().map(BigInt::to_field::<F>).transpose()?;
            Ok((id, (list?, opt)))
        })
        .collect::<Result<_>>()
        .with_context(|| {
            format!(
                "cannot decode circuit file {}",
                internal_vars_path.display()
            )
        })?;

    let rows_rev: Vec<_> = rows_rev
        .iter()
        .map(|row| {
            let row: Vec<_> = row.iter().map(|v| v.as_ref().map(V::from)).collect();
            row
        })
        .collect();

    Ok((internal_vars, rows_rev))
}

fn read_constraints_data<F: FieldWitness>(
    internal_vars_path: &Path,
    rows_rev_path: &Path,
) -> Result<(InternalVars<F>, Vec<Vec<Option<V>>>)> {
    // ((Fp.t * V.t) list * Fp.t option)
    let internal_vars = circuit_blobs::fetch_blocking(&internal_vars_path)
        .with_context(|| format!("cannot load circuit file {}", internal_vars_path.display()))?;

    // V.t option array list
    let rows_rev = circuit_blobs::fetch_blocking(&rows_rev_path)
        .with_context(|| format!("cannot load circuit file {}", rows_rev_path.display()))?;

    decode_constraints_data(
        internal_vars.as_slice(),
        rows_rev.as_slice(),
        internal_vars_path,
        rows_rev_path,
    )
}

/// For tests that need the Mesa mainnet circuit blobs: sets the global
/// network config to mainnet, and checks that `MINA_CIRCUIT_BLOBS_BASE_DIR`
/// holds the blobs, so that a missing blob fails here, loudly, instead of
/// sending the prover to download a release that doesn't exist.
#[cfg(test)]
pub(crate) fn init_mainnet_with_mesa_blobs() -> &'static CircuitsConfig {
    init_with_mesa_blobs("mainnet")
}

/// As [`init_mainnet_with_mesa_blobs`], for the Mesa devnet circuits
/// (`4.0.2devnet`, from `MINA_PROFILE=devnet tools/mesa-circuits/generate.sh`).
#[cfg(test)]
pub(crate) fn init_devnet_with_mesa_blobs() -> &'static CircuitsConfig {
    init_with_mesa_blobs("devnet")
}

#[cfg(test)]
fn init_with_mesa_blobs(network: &str) -> &'static CircuitsConfig {
    let how = format!(
        "needs the Mesa {network} circuit blobs: run{} tools/mesa-circuits/generate.sh <dir>{} \
         and set MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs",
        if network == "devnet" {
            " MINA_PROFILE=devnet"
        } else {
            ""
        },
        if network == "devnet" {
            " tools/mesa-circuits/mesa-devnet-fork.json"
        } else {
            ""
        },
    );

    let _ = mina_core::NetworkConfig::init(network);
    let config = mina_core::NetworkConfig::global();
    assert_eq!(
        config.name, network,
        "another test set the network config first: run this test on its own"
    );
    let config = config.circuits_config;
    #[cfg(feature = "embedded-circuits")]
    if network == "devnet" {
        return config;
    }
    let base = std::env::var("MINA_CIRCUIT_BLOBS_BASE_DIR").expect(&how);
    let gates = Path::new(&base)
        .join(config.directory_name)
        .join(format!("{}_gates.json", config.step_transaction_gates));
    assert!(gates.exists(), "no {}: {how}", gates.display());
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proofs::constants::ProofConstants;

    /// Run alone: the global prover cache may already be initialized by other
    /// tests in the full suite. The injected miss reaches the same native
    /// blob lookup used by both startup constructors.
    #[test]
    #[ignore = "run alone with --features embedded-circuits --ignored"]
    #[cfg(feature = "embedded-circuits")]
    fn a_missing_circuit_returns_producer_and_worker_errors() {
        use crate::proofs::snark_worker::WorkProvers;

        let config = init_devnet_with_mesa_blobs();
        circuit_blobs::with_missing_circuit(|| {
            let producer = BlockProver::make(None, None).err().expect("must fail");
            let producer = format!("{producer:#}");
            assert!(
                producer.contains(config.step_blockchain_gates),
                "{producer}"
            );
            assert!(producer.contains("_gates.json"), "{producer}");
            assert!(producer.contains("test circuit blob missing"), "{producer}");

            let worker = WorkProvers::make(None).err().expect("must fail");
            let worker = format!("{worker:#}");
            assert!(worker.contains(config.step_transaction_gates), "{worker}");
            assert!(worker.contains("_gates.json"), "{worker}");
            assert!(worker.contains("test circuit blob missing"), "{worker}");
        });
    }

    /// The sizes in `constants.rs` against the blobs, for one circuit, as the
    /// prover loads them: gates, rows and the external variables the rows
    /// and internal variables refer to, which are `PRIMARY_LEN + AUX_LEN`.
    fn check_sizes<C: ProofConstants, F: FieldWitness>(name: &str) {
        let (internal_vars, rows_rev, gates) = make_gates::<F>(name).unwrap();
        let max_external = rows_rev
            .iter()
            .flatten()
            .flatten()
            .chain(
                internal_vars
                    .values()
                    .flat_map(|(lc, _)| lc.iter().map(|(_, v)| v)),
            )
            .filter_map(|v| match v {
                V::External(i) => Some(*i),
                V::Internal(_) => None,
            })
            .max()
            .unwrap();
        let sizes = (
            gates.len(),
            rows_rev.len() + C::PRIMARY_LEN,
            max_external + 1,
        );
        let expected = (C::ROWS, C::ROWS, C::PRIMARY_LEN + C::AUX_LEN);
        assert_eq!(sizes, expected, "{name}: (gates, rows, external variables)");
        eprintln!(
            "{name}: ROWS {} and AUX_LEN {} match the blobs",
            C::ROWS,
            C::AUX_LEN
        );
    }

    /// `ROWS` and `AUX_LEN` of all 8 Mesa circuits match the blobs that
    /// `CircuitsConfig` names.
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_circuit_sizes() {
        check_all_sizes(init_mainnet_with_mesa_blobs());
    }

    /// The Mesa devnet circuits have the mainnet circuits' sizes, so the
    /// same `ROWS` and `AUX_LEN` hold for both networks.
    #[test]
    #[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
                tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_devnet_circuit_sizes() {
        let config = init_devnet_with_mesa_blobs();
        assert_eq!(config.directory_name, "4.0.2devnet");
        check_all_sizes(config);
    }

    fn check_all_sizes(config: &CircuitsConfig) {
        check_sizes::<StepTransactionProof, Fp>(config.step_transaction_gates);
        check_sizes::<WrapTransactionProof, Fq>(config.wrap_transaction_gates);
        check_sizes::<StepMergeProof, Fp>(config.step_merge_gates);
        check_sizes::<StepBlockProof, Fp>(config.step_blockchain_gates);
        check_sizes::<WrapBlockProof, Fq>(config.wrap_blockchain_gates);
        check_sizes::<StepZkappOptSignedOptSignedProof, Fp>(
            config.step_transaction_opt_signed_opt_signed_gates,
        );
        check_sizes::<StepZkappOptSignedProof, Fp>(config.step_transaction_opt_signed_gates);
        check_sizes::<StepZkappProvedProof, Fp>(config.step_transaction_proved_gates);
    }
}
