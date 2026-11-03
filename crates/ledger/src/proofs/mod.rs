use field::FieldWitness;
use poly_commitment::ipa::{OpeningProof, SRS};

use mina_poseidon::pasta::FULL_ROUNDS;

pub mod accumulator_check;
pub mod block;
pub mod caching;
pub mod circuit_blobs;
pub mod constants;
mod conv;
pub mod field;
pub mod fork_circuits;
pub mod group_map;
pub mod lagrange_cache;
pub mod merge;
pub mod numbers;
pub mod opt_sponge;
mod prover;
pub mod provers;
pub mod public_input;
pub mod snark_worker;
pub mod step;
pub mod to_field_elements;
pub mod transaction;
pub mod transition_chain;
pub mod unfinalized;
mod urs_utils;
pub mod util;
pub mod verification;
pub mod verifiers;
pub mod witness;
pub mod wrap;
pub mod zkapp;

pub const BACKEND_TICK_ROUNDS_N: usize = 16;
pub const BACKEND_TOCK_ROUNDS_N: usize = 15;

pub type VerifierIndex<F> = kimchi::verifier_index::VerifierIndex<
    FULL_ROUNDS,
    <F as FieldWitness>::OtherCurve,
    SRS<<F as FieldWitness>::OtherCurve>,
>;
pub type ProverIndex<F> = kimchi::prover_index::ProverIndex<
    FULL_ROUNDS,
    <F as FieldWitness>::OtherCurve,
    SRS<<F as FieldWitness>::OtherCurve>,
>;
pub type ProverProof<F> = kimchi::proof::ProverProof<
    <F as FieldWitness>::OtherCurve,
    OpeningProof<<F as FieldWitness>::OtherCurve, FULL_ROUNDS>,
    FULL_ROUNDS,
>;

pub fn generate_tx_proof(
    params: transaction::TransactionParams,
) -> anyhow::Result<wrap::WrapProof> {
    use mina_curves::pasta::Fp;
    use witness::Witness;
    let mut w: Witness<Fp> = Witness::new::<constants::StepTransactionProof>();
    transaction::generate_tx_proof(params, &mut w)
}

pub fn generate_merge_proof(params: merge::MergeParams) -> anyhow::Result<wrap::WrapProof> {
    use mina_curves::pasta::Fp;
    use witness::Witness;
    let mut w: Witness<Fp> = Witness::new::<constants::StepMergeProof>();
    merge::generate_merge_proof(params, &mut w)
}

pub fn generate_block_proof(params: block::BlockParams) -> anyhow::Result<wrap::WrapProof> {
    use mina_curves::pasta::Fp;
    use witness::Witness;
    let mut w: Witness<Fp> = Witness::new::<constants::StepBlockProof>();
    block::generate_block_proof(params, &mut w)
}
pub use zkapp::generate_zkapp_proof;

/// The panic boundary of one proving job: runs `prove`, and turns a panic in
/// it into `Err` with the panic's message, logged as "prover recovered after
/// panic", so that a failed proof costs one job and its thread goes on to the
/// next. The SNARK worker, the uptime prover and the native block prover put
/// each job in one. A panic that held its thread's prover SRS lock poisoned
/// it: the thread's next proof makes it again ([`crate::verifier::get_srs_mut`]).
///
/// A web node's prover cannot unwind (`panic_abort`): a panic there stops its
/// whole instance, which the bridge replaces with a new one.
pub fn catch_prover_panic<T>(prover: &str, prove: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(prove)).map_err(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        PROVER_PANICS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        mina_core::warn!(
            mina_core::log::system_time();
            kind = "ProverRecoveredAfterPanic",
            summary = format!("prover recovered after panic: the {prover} job failed"),
            prover = prover.to_owned(),
            error = message.clone(),
            panics = prover_panics(),
        );
        message
    })
}

static PROVER_PANICS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many proving jobs [`catch_prover_panic`] turned from a panic into an
/// error, in this process.
pub fn prover_panics() -> u64 {
    PROVER_PANICS.load(std::sync::atomic::Ordering::Relaxed)
}
