//! The proving half of a SNARK worker (P5.2, P5.2e): one work spec, proved
//! for one sok message, then checked as the snark pool would check it.
//!
//! An OCaml worker proves `Transition` (base) and `Merge` specs with the
//! sok message of its fee and public key; the daemon then gossips the result
//! as a snark pool diff. OCaml peers verify every diff and punish a sender of
//! an invalid proof (`Sent_invalid_proof`), so our worker verifies its own
//! work before the node offers it.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L319-L476>
//!
//! A zkApp command's base job is proved as OCaml 4.0.2's work partitioner
//! splits it: one proof per segment, with its zkApp step circuit and the
//! transaction wrap, and the segments' proofs merged in order into one
//! (P5.1's `generate_zkapp_proof`). The zkApp step provers are made on the
//! first zkApp job, not when the worker starts.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_partitioner/work_partitioner.ml#L133-L166>

use std::time::Duration;

use mina_core::snark::Snark;
use mina_curves::pasta::{Fq, Vesta};
use mina_p2p_messages::v2::{
    self, SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Instances as WorkSpec,
    SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single as SingleSpec,
};
use once_cell::sync::OnceCell;
use poly_commitment::ipa::SRS;

use super::{
    merge::MergeParams,
    provers::{TransactionProver, ZkappProver},
    transaction::TransactionParams,
    verifiers::TransactionVerifier,
    zkapp::ZkappParams,
    VerifierIndex,
};
use crate::{
    scan_state::scan_state::transaction_snark::{SokMessage, Statement},
    verifier::snark_work::{verify_snark_works, SnarkWorkError},
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkProveError {
    #[error("invalid statement in the work spec")]
    InvalidStatement,
    #[error("the zkApp proof's statement is not the job's")]
    ZkappStatementMismatch,
    #[error("proving failed: {0}")]
    Prove(String),
    #[error("cannot load prover circuits: {0}")]
    Load(String),
}

/// The provers a worker uses: the transaction and merge provers from the
/// start, the zkApp step provers from its first zkApp command job.
pub struct WorkProvers {
    pub transaction: TransactionProver,
    verifier: Option<TransactionVerifier>,
    zkapp: OnceCell<ZkappProver>,
}

impl WorkProvers {
    pub fn make(verifier: Option<TransactionVerifier>) -> anyhow::Result<Self> {
        Ok(Self {
            transaction: TransactionProver::make(verifier.clone())?,
            verifier,
            zkapp: OnceCell::new(),
        })
    }

    pub fn zkapp_loaded(&self) -> bool {
        self.zkapp.get().is_some()
    }

    /// The zkApp provers, made on first use: the three zkApp step circuits;
    /// the merge and wrap provers are the transaction ones.
    pub fn zkapp(&self) -> anyhow::Result<&ZkappProver> {
        self.zkapp
            .get_or_try_init(|| ZkappProver::make(self.verifier.clone()))
    }
}

/// What a job of a spec is, for logs and time limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Base,
    /// A zkApp command's base job, of this many segments.
    Zkapp {
        segments: usize,
    },
    Merge,
}

impl JobKind {
    pub fn of(single: &SingleSpec) -> Self {
        match single {
            SingleSpec::Transition(_, witness) => Self::of_transaction(&witness.transaction),
            SingleSpec::Merge(_) => Self::Merge,
        }
    }

    pub fn of_transaction(transaction: &v2::MinaTransactionTransactionStableV2) -> Self {
        match zkapp_command(transaction) {
            Some(command) => Self::Zkapp {
                segments: zkapp_segments(command),
            },
            None => Self::Base,
        }
    }

    /// The proofs a worker makes for the job, one after another: for a
    /// zkApp command one per segment and one per merge of two.
    pub fn proofs(&self) -> usize {
        match self {
            Self::Zkapp { segments } => segments.saturating_mul(2).saturating_sub(1).max(1),
            Self::Base | Self::Merge => 1,
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base => write!(f, "base"),
            Self::Zkapp { segments } => write!(f, "zkapp/{segments}"),
            Self::Merge => write!(f, "merge"),
        }
    }
}

fn zkapp_command(
    transaction: &v2::MinaTransactionTransactionStableV2,
) -> Option<&v2::MinaBaseZkappCommandTStableV1WireStableV1> {
    match transaction {
        v2::MinaTransactionTransactionStableV2::Command(command) => match command.as_ref() {
            v2::MinaBaseUserCommandStableV2::ZkappCommand(zkapp) => Some(zkapp),
            v2::MinaBaseUserCommandStableV2::SignedCommand(_) => None,
        },
        _ => None,
    }
}

/// The segments a zkApp command is proved in, which OCaml calls the
/// command's cost (`zkapp_cost`, at most 16 by `valid_size`): its account
/// updates, fee payer first and then the call forest in pre-order, grouped
/// as `group_by_zkapp_command_rev` groups one command. An update authorised
/// by a proof is a segment alone, and so is one followed by such an update;
/// otherwise two consecutive updates pair up.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_command.ml#L1014-L1285>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_command.ml#L1312-L1391>
pub fn zkapp_segments(command: &v2::MinaBaseZkappCommandTStableV1WireStableV1) -> usize {
    fn push(
        tree: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
        proved: &mut Vec<bool>,
    ) {
        proved.push(matches!(
            tree.account_update.authorization,
            v2::MinaBaseControlStableV2::Proof(_)
        ));
        for call in tree.calls.iter() {
            push(&call.elt, proved);
        }
    }
    // The fee payer signs.
    let mut proved = vec![false];
    for update in command.account_updates.iter() {
        push(&update.elt, &mut proved);
    }
    let (mut segments, mut i) = (0, 0);
    while i < proved.len() {
        segments += 1;
        i += if !proved[i] && proved.get(i + 1) == Some(&false) {
            2
        } else {
            1
        };
    }
    segments
}

/// Whether a single spec is a zkApp command's base job.
pub fn is_zkapp_command(single: &SingleSpec) -> bool {
    matches!(JobKind::of(single), JobKind::Zkapp { .. })
}

/// The jobs of a spec, one or two.
pub fn spec_jobs(spec: &WorkSpec) -> Vec<&SingleSpec> {
    match spec {
        WorkSpec::One(single) => vec![single],
        WorkSpec::Two((first, second)) => vec![first, second],
    }
}

/// A proved spec: its proofs, and how long each one took (step and wrap,
/// witness included; for a zkApp command every segment and merge).
#[derive(Debug, Clone)]
pub struct ProvedWork {
    pub proofs: v2::TransactionSnarkWorkTStableV2Proofs,
    pub prove_times: Vec<Duration>,
}

/// Proves every job of `spec` for `message`, as OCaml's worker does
/// (`Transaction_snark.of_transition` for a base job, the zkApp segments
/// and their merges for a zkApp command, `merge` for a merge). The first
/// job that fails ends it, with nothing proved offered.
pub fn prove_work(
    spec: &WorkSpec,
    message: &SokMessage,
    provers: &WorkProvers,
) -> Result<ProvedWork, WorkProveError> {
    let singles = spec_jobs(spec);
    let mut proofs = Vec::with_capacity(singles.len());
    let mut prove_times = Vec::with_capacity(singles.len());
    for single in singles {
        // redux's clock: `std::time::Instant` panics in a browser (P5.3).
        let started = redux::Instant::now();
        proofs.push(prove_single(single, message, provers)?);
        prove_times.push(started.elapsed());
    }
    let mut proofs = proofs.into_iter();
    let proofs = match (proofs.next(), proofs.next()) {
        (Some(one), None) => v2::TransactionSnarkWorkTStableV2Proofs::One(one),
        (Some(one), Some(two)) => v2::TransactionSnarkWorkTStableV2Proofs::Two((one, two)),
        _ => unreachable!("a work spec has one or two jobs"),
    };
    Ok(ProvedWork {
        proofs,
        prove_times,
    })
}

fn prove_single(
    single: &SingleSpec,
    message: &SokMessage,
    provers: &WorkProvers,
) -> Result<v2::LedgerProofProdStableV2, WorkProveError> {
    let transaction = &provers.transaction;
    let (statement, proof) = match single {
        SingleSpec::Transition(statement, witness) if is_zkapp_command(single) => {
            let zkapp = provers
                .zkapp()
                .map_err(|err| WorkProveError::Load(format!("{err:#}")))?;
            return prove_zkapp(statement, witness, message, zkapp);
        }
        SingleSpec::Transition(statement, witness) => {
            let proof = super::generate_tx_proof(TransactionParams {
                statement: &statement.0,
                tx_witness: witness,
                message,
                tx_step_prover: &transaction.tx_step_prover,
                tx_wrap_prover: &transaction.tx_wrap_prover,
                only_verify_constraints: false,
                expected_step_proof: None,
                ocaml_wrap_witness: None,
            });
            (&statement.0, proof)
        }
        SingleSpec::Merge(merge) => {
            let (statement, left, right) = merge.as_ref();
            let proof = super::generate_merge_proof(MergeParams {
                statement: (&statement.0)
                    .try_into()
                    .map_err(|_| WorkProveError::InvalidStatement)?,
                proofs: &[left.clone(), right.clone()],
                message,
                step_prover: &transaction.merge_step_prover,
                wrap_prover: &transaction.tx_wrap_prover,
                only_verify_constraints: false,
                expected_step_proof: None,
                ocaml_wrap_witness: None,
            });
            (&statement.0, proof)
        }
    };
    let proof = proof.map_err(|err| WorkProveError::Prove(err.to_string()))?;
    Ok(v2::LedgerProofProdStableV2(v2::TransactionSnarkStableV2 {
        statement: v2::MinaStateSnarkedLedgerStateWithSokStableV2 {
            source: statement.source.clone(),
            target: statement.target.clone(),
            connecting_ledger_left: statement.connecting_ledger_left.clone(),
            connecting_ledger_right: statement.connecting_ledger_right.clone(),
            supply_increase: statement.supply_increase.clone(),
            fee_excess: statement.fee_excess.clone(),
            sok_digest: (&message.digest()).into(),
        },
        proof: v2::TransactionSnarkProofStableV2((&proof).into()).into(),
    }))
}

/// A zkApp command's segments, proved and merged into one proof for the
/// job's statement. A witness that fails one of a circuit's assertions is
/// an error (P5.1), and the segments after it are not proved.
fn prove_zkapp(
    statement: &v2::MinaStateSnarkedLedgerStateStableV2,
    witness: &v2::TransactionWitnessStableV2,
    message: &SokMessage,
    provers: &ZkappProver,
) -> Result<v2::LedgerProofProdStableV2, WorkProveError> {
    let expected =
        Statement::<()>::try_from(&statement.0).map_err(|_| WorkProveError::InvalidStatement)?;
    let proof = super::generate_zkapp_proof(ZkappParams {
        statement: &statement.0,
        tx_witness: witness,
        message,
        step_opt_signed_opt_signed_prover: &provers.step_opt_signed_opt_signed_prover,
        step_opt_signed_prover: &provers.step_opt_signed_prover,
        step_proof_prover: &provers.step_proof_prover,
        merge_step_prover: &provers.merge_step_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        opt_signed_path: None,
        proved_path: None,
    })
    .map_err(|err| WorkProveError::Prove(format!("{err:#}")))?;
    // The merged statement is the job's by construction; a proof of any
    // other would be refused by every pool, ours first.
    if proof.statement.sok_digest != message.digest()
        || proof.statement.clone().without_digest() != expected
    {
        return Err(WorkProveError::ZkappStatementMismatch);
    }
    Ok((&proof).into())
}

/// Checks our own work as an OCaml snark pool checks a diff: every proof's
/// sok digest is the digest of the work's fee and prover, and every proof
/// verifies with this network's transaction key.
pub fn check_work(
    work: &Snark,
    verifier_index: &VerifierIndex<Fq>,
    srs: &SRS<Vesta>,
) -> Result<(), SnarkWorkError> {
    verify_snark_works(std::slice::from_ref(work), verifier_index, srs)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use mina_p2p_messages::{
        binprot::{BinProtRead, BinProtWrite},
        gossip::{decode_pubsub_data, GossipNetMessageV2},
    };
    use mina_signer::CompressedPubKey;

    use super::*;
    use crate::{
        proofs::{
            provers::{init_devnet_with_mesa_blobs, init_mainnet_with_mesa_blobs},
            transaction::mesa_tests::PROVING,
        },
        scan_state::currency::Fee,
        verifier::get_srs,
    };

    type BaseFixture = (
        v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        v2::TransactionWitnessStableV2,
    );
    type MergeFixture = (
        v2::MinaStateSnarkedLedgerStateStableV2,
        v2::LedgerProofProdStableV2,
        v2::LedgerProofProdStableV2,
    );

    // The P4.3, P4.4 and P4.8 fixtures, OCaml's own jobs from Mesa scan
    // states; their provenance is documented in `transaction.rs`'s
    // `mesa_tests`.
    const MAINNET_PAYMENT: &[u8] = include_bytes!("data/mesa_mainnet_payment_base_job.bin");
    const MAINNET_MERGE_OF_BASES: &[u8] =
        include_bytes!("data/mesa_mainnet_merge_of_bases_job.bin");
    const DEVNET_MERGE_LEFT: &[u8] =
        include_bytes!("data/mesa_devnet_merge_of_bases_left_base_job.bin");
    const DEVNET_MERGE_RIGHT: &[u8] =
        include_bytes!("data/mesa_devnet_merge_of_bases_right_base_job.bin");
    const DEVNET_MERGE_OF_BASES: &[u8] = include_bytes!("data/mesa_devnet_merge_of_bases_job.bin");
    // P5.1's zkApp command base jobs, OCaml's own from Mesa scan states; their
    // provenance is documented in `zkapp/mesa_tests.rs`.
    const MAINNET_ZKAPP_SIGNED: &[u8] =
        include_bytes!("data/mesa_mainnet_zkapp_signed_base_job.bin");
    const MAINNET_ZKAPP_PROVED: &[u8] =
        include_bytes!("data/mesa_mainnet_zkapp_proved_base_job.bin");
    const MAINNET_ZKAPP_TWO_PROOFS: &[u8] =
        include_bytes!("data/mesa_mainnet_zkapp_two_proofs_base_job.bin");
    const DEVNET_ZKAPP_PROVED: &[u8] = include_bytes!("data/mesa_devnet_zkapp_proved_base_job.bin");
    const DEVNET_ZKAPP_SIGNED: &[u8] = include_bytes!("data/mesa_devnet_zkapp_signed_base_job.bin");
    const DEVNET_ZKAPP_SIGNED_R39: &[u8] =
        include_bytes!("data/mesa_devnet_zkapp_signed_r39_base_job.bin");

    /// The public genesis winner of Mina's ledgers, as our worker's fee key.
    const PROVER: &str = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";

    fn base_spec(bytes: &[u8]) -> SingleSpec {
        let (statement, witness) = BaseFixture::binprot_read(&mut &*bytes).unwrap();
        SingleSpec::Transition(v2::MinaStateSnarkedLedgerStateStableV2(statement), witness)
    }

    fn merge_spec(bytes: &[u8]) -> SingleSpec {
        let merge = MergeFixture::binprot_read(&mut &*bytes).unwrap();
        SingleSpec::Merge(Box::new(merge))
    }

    fn fee(nanomina: u64) -> v2::CurrencyFeeStableV1 {
        v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
            nanomina.into(),
        ))
    }

    /// Replays an operator capture without a running node. The file contains
    /// the entire wire work spec followed by its SOK message.
    #[test]
    #[ignore = "set MINA_ZK1_CAPTURE to a failed devnet work capture"]
    fn reprove_captured_zkapp_failure() {
        let path = std::env::var("MINA_ZK1_CAPTURE").expect("capture path");
        let mut file = std::fs::File::open(path).unwrap();
        let spec = WorkSpec::binprot_read(&mut file).unwrap();
        let sok = v2::MinaBaseSokMessageStableV1::binprot_read(&mut file).unwrap();
        let message = SokMessage::try_from(&sok).unwrap();
        if let Ok(dir) = std::env::var("MINA_ZK1_EXTRACT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                format!("{dir}/capture.json"),
                serde_json::to_vec_pretty(&spec).unwrap(),
            )
            .unwrap();
            for (index, single) in spec_jobs(&spec).into_iter().enumerate() {
                if let SingleSpec::Transition(statement, witness) = single {
                    if is_zkapp_command(single) {
                        let fixture: BaseFixture = (statement.0.clone(), witness.clone());
                        let mut out =
                            std::fs::File::create(format!("{dir}/zkapp-{index}.bin")).unwrap();
                        fixture.binprot_write(&mut out).unwrap();
                    }
                }
            }
        }
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_devnet_with_mesa_blobs();
        let provers = WorkProvers::make(Some(TransactionVerifier::make())).unwrap();
        for attempt in 1..=2 {
            let result = prove_work(&spec, &message, &provers);
            eprintln!(
                "capture replay {attempt}: {}",
                match result {
                    Ok(_) => "proved".to_owned(),
                    Err(error) => error.to_string(),
                }
            );
        }
    }

    /// The feature flags of the proofs authorising a zkApp command's account
    /// updates, as the names of the enabled ones: a circuit using custom
    /// gates or lookups has some (ZK1).
    fn proof_feature_flags(single: &SingleSpec) -> Vec<Vec<&'static str>> {
        fn push(
            tree: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
            flags: &mut Vec<Vec<&'static str>>,
        ) {
            if let v2::MinaBaseControlStableV2::Proof(proof) = &tree.account_update.authorization {
                let decoded = proof.get().unwrap();
                let f = &decoded
                    .statement
                    .proof_state
                    .deferred_values
                    .plonk
                    .feature_flags;
                flags.push(
                    [
                        ("range_check0", f.range_check0),
                        ("range_check1", f.range_check1),
                        ("foreign_field_add", f.foreign_field_add),
                        ("foreign_field_mul", f.foreign_field_mul),
                        ("xor", f.xor),
                        ("rot", f.rot),
                        ("lookup", f.lookup),
                        ("runtime_tables", f.runtime_tables),
                    ]
                    .into_iter()
                    .filter_map(|(name, on)| on.then_some(name))
                    .collect(),
                );
            }
            for call in tree.calls.iter() {
                push(&call.elt, flags);
            }
        }
        let mut flags = Vec::new();
        if let SingleSpec::Transition(_, witness) = single {
            if let Some(command) = super::zkapp_command(&witness.transaction) {
                for update in command.account_updates.iter() {
                    push(&update.elt, &mut flags);
                }
            }
        }
        flags
    }

    /// The authorisations of a zkApp command job's account updates, fee
    /// payer first, as the fixture extractor names shapes: `S` signature,
    /// `N` none, `P` proof.
    fn authorization_shape(single: &SingleSpec) -> String {
        fn push(
            tree: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
            shape: &mut String,
        ) {
            shape.push(match &tree.account_update.authorization {
                v2::MinaBaseControlStableV2::Signature(_) => 'S',
                v2::MinaBaseControlStableV2::NoneGiven => 'N',
                v2::MinaBaseControlStableV2::Proof(_) => 'P',
            });
            for call in tree.calls.iter() {
                push(&call.elt, shape);
            }
        }
        let mut shape = String::from("S");
        if let SingleSpec::Transition(_, witness) = single {
            if let Some(command) = super::zkapp_command(&witness.transaction) {
                for update in command.account_updates.iter() {
                    push(&update.elt, &mut shape);
                }
            }
        }
        shape
    }

    /// ZK1's live audit: reads every worker capture in `MINA_ZK1_CAPTURE_DIR`
    /// (failed jobs, and with `MINA_SNARK_WORKER_CAPTURE_ZKAPP` every zkApp
    /// job that proved) and prints each zkApp job's shape and the feature
    /// flags of its proofs, with totals. `MINA_ZK1_EXTRACT_DIR` also writes
    /// the zkApp jobs as fixtures for `reprove_historical_zkapp_jobs`. No
    /// blobs needed.
    #[test]
    #[ignore = "set MINA_ZK1_CAPTURE_DIR to a worker capture directory"]
    fn summarize_captured_zkapp_jobs() {
        let dir = std::env::var("MINA_ZK1_CAPTURE_DIR").expect("capture directory");
        let extract = std::env::var("MINA_ZK1_EXTRACT_DIR").ok();
        let mut paths = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
            .collect::<Vec<_>>();
        paths.sort();
        let (mut jobs, mut with_proofs, mut proofs, mut with_features) = (0, 0, 0, 0);
        for path in &paths {
            let name = path.file_name().unwrap().to_string_lossy();
            let mut file = std::fs::File::open(path).unwrap();
            let spec = WorkSpec::binprot_read(&mut file).unwrap();
            for (index, single) in spec_jobs(&spec).into_iter().enumerate() {
                let SingleSpec::Transition(statement, witness) = single else {
                    continue;
                };
                if !is_zkapp_command(single) {
                    continue;
                }
                let shape = authorization_shape(single);
                let flags = proof_feature_flags(single);
                jobs += 1;
                with_proofs += usize::from(!flags.is_empty());
                proofs += flags.len();
                with_features += flags.iter().filter(|flags| !flags.is_empty()).count();
                eprintln!("captured {name} job {index}: {shape}, proofs' feature flags {flags:?}");
                if let Some(dir) = &extract {
                    std::fs::create_dir_all(dir).unwrap();
                    let stem = name.trim_end_matches(".bin");
                    let fixture: BaseFixture = (statement.0.clone(), witness.clone());
                    let mut out =
                        std::fs::File::create(format!("{dir}/{stem}-{index}-{shape}.bin")).unwrap();
                    fixture.binprot_write(&mut out).unwrap();
                }
            }
        }
        eprintln!(
            "captures: {} files, {jobs} zkApp jobs, {with_proofs} with proof-authorised \
             updates, {proofs} proofs, {with_features} of them with feature flags",
            paths.len()
        );
    }

    /// ZK1's screen: proves each chain-replayed base job in
    /// `MINA_ZK1_HISTORICAL_DIR` (fixture files; `MINA_ZK1_HEIGHT` keeps
    /// one block's) as the node's worker does, and checks what the snark
    /// pool checks: the proof is of the job's statement for our sok digest
    /// and verifies with the devnet key. It prints the feature flags of each
    /// job's zkApp proofs and fails if any job does. `MINA_ZK1_JSON_DIR`
    /// also writes each fixture as JSON, and with `MINA_ZK1_DUMP_ONLY` does
    /// only that.
    #[test]
    #[ignore = "set MINA_ZK1_HISTORICAL_DIR to the replay's fixture directory"]
    fn reprove_historical_zkapp_jobs() {
        let dir = std::env::var("MINA_ZK1_HISTORICAL_DIR").expect("historical fixture directory");
        let height = std::env::var("MINA_ZK1_HEIGHT").ok();
        let mut paths = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "bin")
                    && height.as_ref().is_none_or(|height| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .contains(&format!("-{height}-"))
                    })
            })
            .collect::<Vec<_>>();
        paths.sort();
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_devnet_with_mesa_blobs();
        let verifier = TransactionVerifier::make();
        let srs = get_srs::<mina_curves::pasta::Fp>();
        let provers = WorkProvers::make(Some(verifier.clone())).unwrap();
        let message = SokMessage::create(
            Fee::from_u64(100_000_000),
            CompressedPubKey::from_address(PROVER).unwrap(),
        );
        let mut failed = Vec::new();
        for path in &paths {
            let name = path.file_name().unwrap().to_string_lossy();
            let bytes = std::fs::read(path).unwrap();
            if let Ok(dir) = std::env::var("MINA_ZK1_JSON_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                let fixture = BaseFixture::binprot_read(&mut &*bytes).unwrap();
                std::fs::write(
                    format!("{dir}/{name}.json"),
                    serde_json::to_vec_pretty(&fixture).unwrap(),
                )
                .unwrap();
                if std::env::var_os("MINA_ZK1_DUMP_ONLY").is_some() {
                    continue;
                }
            }
            let single = base_spec(&bytes);
            let features = proof_feature_flags(&single);
            let expected = match &single {
                SingleSpec::Transition(statement, _) => Statement::<()>::try_from(&statement.0)
                    .unwrap()
                    .with_digest(message.digest()),
                SingleSpec::Merge(_) => unreachable!("a base job"),
            };
            let started = Instant::now();
            let outcome = prove_work(&WorkSpec::One(single), &message, &provers)
                .map_err(|error| error.to_string())
                .and_then(|proved| {
                    let v2::TransactionSnarkWorkTStableV2Proofs::One(proof) = &proved.proofs else {
                        unreachable!("one job, one proof")
                    };
                    let statement = Statement::try_from(&proof.0.statement)
                        .map_err(|error| format!("{error:?}"))?;
                    if statement != expected {
                        return Err("the proof is of another statement".to_owned());
                    }
                    let work = Snark {
                        snarker: PROVER.parse().unwrap(),
                        fee: fee(100_000_000),
                        proofs: proved.proofs.into(),
                    };
                    check_work(&work, &verifier, &srs).map_err(|error| format!("{error:?}"))
                });
            eprintln!(
                "historical {name} {:.1}s proofs' feature flags {features:?}: {}",
                started.elapsed().as_secs_f64(),
                match &outcome {
                    Ok(()) => "proved and verified".to_owned(),
                    Err(error) => error.clone(),
                }
            );
            if outcome.is_err() {
                failed.push(name.into_owned());
            }
        }
        eprintln!(
            "historical screen: {} of {} jobs proved and verified",
            paths.len() - failed.len(),
            paths.len()
        );
        assert!(failed.is_empty(), "failed: {failed:?}");
    }

    /// Proves `spec` as the node's worker does, for fee 0.1 MINA, and checks
    /// what the snark pool and OCaml peers check: the proofs verify with this
    /// network's key for our sok digest and are refused for another fee's;
    /// the gossip diff survives OCaml's decoder unchanged.
    fn prove_check_and_gossip(spec: WorkSpec) -> Snark {
        let verifier = TransactionVerifier::make();
        prove_check_and_gossip_with(spec, &WorkProvers::make(Some(verifier)).unwrap())
    }

    fn prove_check_and_gossip_with(spec: WorkSpec, provers: &WorkProvers) -> Snark {
        let verifier = TransactionVerifier::make();
        let srs = get_srs::<mina_curves::pasta::Fp>();
        let prover = CompressedPubKey::from_address(PROVER).unwrap();
        let message = SokMessage::create(Fee::from_u64(100_000_000), prover);

        let kinds: Vec<_> = match &spec {
            WorkSpec::One(one) => vec![JobKind::of(one)],
            WorkSpec::Two((one, two)) => vec![JobKind::of(one), JobKind::of(two)],
        };
        let started = Instant::now();
        let proved = prove_work(&spec, &message, provers).unwrap();
        eprintln!(
            "D5 work kinds={kinds:?} prove_seconds={:?} wall_seconds={:.3}",
            proved
                .prove_times
                .iter()
                .map(Duration::as_secs_f64)
                .collect::<Vec<_>>(),
            started.elapsed().as_secs_f64()
        );
        let jobs = match &spec {
            WorkSpec::One(_) => 1,
            WorkSpec::Two(_) => 2,
        };
        assert_eq!(proved.prove_times.len(), jobs);
        assert_eq!(
            matches!(
                proved.proofs,
                v2::TransactionSnarkWorkTStableV2Proofs::Two(_)
            ),
            jobs == 2
        );
        let work = Snark {
            snarker: PROVER.parse().unwrap(),
            fee: fee(100_000_000),
            proofs: proved.proofs.into(),
        };
        check_work(&work, &verifier, &srs).expect("our own work verifies");

        // The same proofs advertised for another fee are someone else's work.
        let resold = Snark {
            fee: fee(99_000_000),
            ..work.clone()
        };
        assert_eq!(
            check_work(&resold, &verifier, &srs),
            Err(SnarkWorkError::SokDigestMismatch { work: 0 })
        );

        // The diff a node gossips: OCaml reads the 8-byte size header, then
        // `Gossip_net.Message.V3.msg`; the decoder is P11's OCaml-exact one.
        let message = GossipNetMessageV2::SnarkPoolDiff {
            message: work.gossip_diff().unwrap(),
            nonce: 0u64.into(),
        };
        let mut body = Vec::new();
        message.binprot_write(&mut body).unwrap();
        let mut data = (body.len() as u64).to_le_bytes().to_vec();
        data.extend_from_slice(&body);
        let decoded = decode_pubsub_data(&data).unwrap();
        assert_eq!(decoded, message);
        let mut again = Vec::new();
        decoded.binprot_write(&mut again).unwrap();
        assert_eq!(again, body);
        work
    }

    /// A Mesa mainnet payment's base job, proved and checked by the worker.
    #[test]
    #[ignore = "needs the Mesa mainnet circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn worker_proves_a_mesa_mainnet_payment() {
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_mainnet_with_mesa_blobs();
        prove_check_and_gossip(WorkSpec::One(base_spec(MAINNET_PAYMENT)));
    }

    /// The first Mesa mainnet merge, over OCaml's two base proofs.
    #[test]
    #[ignore = "needs the Mesa mainnet circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn worker_proves_a_mesa_mainnet_merge() {
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_mainnet_with_mesa_blobs();
        prove_check_and_gossip(WorkSpec::One(merge_spec(MAINNET_MERGE_OF_BASES)));
    }

    /// Devnet, as the gates run: the two base jobs under the devnet merge job,
    /// leaves 127 and 128 of their tree, are the bundle OCaml's
    /// `One_or_two.group_list` makes of them; then the merge of OCaml's two
    /// devnet proofs of those jobs.
    #[test]
    #[ignore = "run with --features embedded-circuits, or provide Mesa devnet blobs"]
    fn worker_proves_a_mesa_devnet_bundle_and_merge() {
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_devnet_with_mesa_blobs();
        let bases = prove_check_and_gossip(WorkSpec::Two((
            base_spec(DEVNET_MERGE_LEFT),
            base_spec(DEVNET_MERGE_RIGHT),
        )));
        let merge = prove_check_and_gossip(WorkSpec::One(merge_spec(DEVNET_MERGE_OF_BASES)));
        // OCaml's merge job is over these two base jobs: our merge statement
        // is the merge of our two base statements.
        let statements = |work: &Snark| match work.statement() {
            v2::TransactionSnarkWorkStatementStableV2::One(one) => vec![one],
            v2::TransactionSnarkWorkStatementStableV2::Two((one, two)) => vec![one, two],
        };
        use crate::scan_state::scan_state::transaction_snark::Statement;
        let [left, right] = &statements(&bases)[..] else {
            panic!("two base statements")
        };
        let merged = Statement::<()>::try_from(&left.0)
            .unwrap()
            .merge(&Statement::<()>::try_from(&right.0).unwrap())
            .unwrap();
        assert_eq!(
            statements(&merge),
            vec![v2::MinaStateSnarkedLedgerStateStableV2((&merged).into())]
        );
    }

    fn zkapp_command(single: &SingleSpec) -> &v2::MinaBaseZkappCommandTStableV1WireStableV1 {
        let SingleSpec::Transition(_, witness) = single else {
            panic!("not a base job")
        };
        super::zkapp_command(&witness.transaction).expect("a zkApp command")
    }

    /// Each job's kind, and the segments of a zkApp command as OCaml groups
    /// them (`zkapp_cost`). Expected counts: P5.1's segments for its fixtures,
    /// whose proofs have OCaml's statements, and mainnet 551243's 16; the
    /// made-up shapes follow `group_by_zkapp_command_rev`'s cases. No blobs.
    #[test]
    fn job_kinds_and_zkapp_segments() {
        assert_eq!(JobKind::of(&base_spec(MAINNET_PAYMENT)), JobKind::Base);
        assert_eq!(
            JobKind::of(&merge_spec(MAINNET_MERGE_OF_BASES)),
            JobKind::Merge
        );
        for (name, job, segments) in [
            ("mainnet signed, SSN", MAINNET_ZKAPP_SIGNED, 2),
            ("mainnet proved, SSSSPS", MAINNET_ZKAPP_PROVED, 4),
            ("mainnet two proofs, SSPPSNN", MAINNET_ZKAPP_TWO_PROOFS, 5),
            ("devnet proved, SP", DEVNET_ZKAPP_PROVED, 2),
            ("devnet signed, SSN", DEVNET_ZKAPP_SIGNED, 2),
            ("devnet signed R39, SSN", DEVNET_ZKAPP_SIGNED_R39, 2),
        ] {
            let single = base_spec(job);
            assert_eq!(JobKind::of(&single), JobKind::Zkapp { segments }, "{name}");
            assert!(is_zkapp_command(&single), "{name}");
            assert_eq!(JobKind::of(&single).proofs(), 2 * segments - 1, "{name}");
        }
        assert!(!is_zkapp_command(&base_spec(MAINNET_PAYMENT)));
        assert_eq!(JobKind::Base.proofs(), 1);
        assert_eq!(JobKind::Merge.proofs(), 1);

        // Shapes made of the two-proofs command's account updates (nested
        // ones included, their calls dropped): a signed one, a proved one and
        // an unauthorised one, fee payer first.
        let template = zkapp_command(&base_spec(MAINNET_ZKAPP_TWO_PROOFS)).clone();
        fn flatten(
            tree: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
            out: &mut Vec<v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA>,
        ) {
            let mut alone = tree.clone();
            alone.calls = mina_p2p_messages::list::List::new();
            out.push(alone);
            for call in tree.calls.iter() {
                flatten(&call.elt, out);
            }
        }
        let mut updates = Vec::new();
        for update in template.account_updates.iter() {
            flatten(&update.elt, &mut updates);
        }
        let pick = |proved: bool, signed: bool| {
            let elt = updates
                .iter()
                .find(|update| match &update.account_update.authorization {
                    v2::MinaBaseControlStableV2::Proof(_) => proved,
                    v2::MinaBaseControlStableV2::Signature(_) => !proved && signed,
                    v2::MinaBaseControlStableV2::NoneGiven => !proved && !signed,
                })
                .expect("an update of that kind")
                .clone();
            v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA {
                elt,
                stack_hash: (),
            }
        };
        let shape = |shape: &str| {
            let mut command = template.clone();
            command.account_updates = shape
                .chars()
                .map(|c| match c {
                    'S' => pick(false, true),
                    'N' => pick(false, false),
                    'P' => pick(true, false),
                    _ => unreachable!(),
                })
                .collect();
            zkapp_segments(&command)
        };
        // After the fee payer: 31 updates without proofs, mainnet 551243's
        // shape, pair into 16 segments.
        assert_eq!(shape(&"N".repeat(31)), 16);
        assert_eq!(shape(""), 1);
        assert_eq!(shape("N"), 1);
        assert_eq!(shape("NN"), 2);
        // The fee payer is alone before a proof; a proof is alone.
        assert_eq!(shape("P"), 2);
        assert_eq!(shape("PP"), 3);
        assert_eq!(shape("PSNP"), 4);
        assert_eq!(shape("SPN"), 3);
    }

    /// The zkApp step provers are made on the first zkApp job, not with the
    /// worker; their cost is measured here (time, and this process's RSS).
    fn load_zkapp_provers(provers: &WorkProvers) {
        assert!(!provers.zkapp_loaded());
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix("VmRSS:"))
                        .and_then(|kib| kib.trim().trim_end_matches(" kB").parse::<u64>().ok())
                })
        };
        let (before, started) = (rss(), Instant::now());
        provers.zkapp().unwrap();
        eprintln!(
            "zkApp provers made in {:.1?}; RSS {before:?} -> {:?} KiB",
            started.elapsed(),
            rss()
        );
        assert!(provers.zkapp_loaded());
    }

    /// The fixture's job with its fee payer's signature changed: the first
    /// segment's witness fails the circuit's assertion that it verifies.
    fn with_bad_fee_payer_signature(job: &[u8]) -> SingleSpec {
        let SingleSpec::Transition(statement, mut witness) = base_spec(job) else {
            unreachable!()
        };
        let v2::MinaTransactionTransactionStableV2::Command(command) = &mut witness.transaction
        else {
            unreachable!()
        };
        let v2::MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = &mut **command else {
            unreachable!()
        };
        let signature = zkapp.fee_payer.authorization.inner().clone();
        let s: Fq = signature.1.to_field().unwrap();
        zkapp.fee_payer.authorization =
            v2::MinaBaseSignatureStableV1(signature.0, (s + Fq::from(1u64)).into()).into();
        SingleSpec::Transition(statement, witness)
    }

    /// A zkApp job as the worker proves it on Mesa mainnet: 548185's command
    /// (`SS SS P S`, a proof verified in the circuit), bundled with the
    /// payment job, so one bundle mixes both kinds. Then the failure path: a
    /// job whose witness fails a circuit assertion is an error, not a panic,
    /// and nothing is proved for its bundle.
    #[test]
    #[ignore = "needs the Mesa mainnet circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn worker_proves_a_mesa_mainnet_zkapp_job() {
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_mainnet_with_mesa_blobs();
        let provers = WorkProvers::make(Some(TransactionVerifier::make())).unwrap();
        load_zkapp_provers(&provers);
        let zkapp = base_spec(MAINNET_ZKAPP_PROVED);
        let SingleSpec::Transition(statement, _) = &zkapp else {
            unreachable!()
        };
        let statement = statement.clone();
        let work = prove_check_and_gossip_with(
            WorkSpec::Two((base_spec(MAINNET_PAYMENT), zkapp)),
            &provers,
        );
        // Our proof is of the scan state's job (OCaml's statement, P5.1).
        let v2::TransactionSnarkWorkStatementStableV2::Two((_, proved)) = work.statement() else {
            panic!("two statements")
        };
        assert_eq!(proved, statement);

        let failing = WorkSpec::Two((
            base_spec(MAINNET_PAYMENT),
            with_bad_fee_payer_signature(MAINNET_ZKAPP_SIGNED),
        ));
        let message = SokMessage::create(
            Fee::from_u64(100_000_000),
            CompressedPubKey::from_address(PROVER).unwrap(),
        );
        let started = Instant::now();
        match prove_work(&failing, &message, &provers) {
            Err(WorkProveError::Prove(why)) => {
                assert!(why.contains("not signed"), "{why}");
            }
            other => panic!("expected the circuit's assertion to fail: {other:?}"),
        }
        eprintln!("failing bundle refused in {:.1?}", started.elapsed());
    }

    /// The same on devnet, as the gate runs: P4.15's command (`SS S`), and
    /// 577542's (`S P`, a key of wrap domain 2^14).
    #[test]
    #[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
                tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn worker_proves_mesa_devnet_zkapp_jobs() {
        let _turn = PROVING.lock().unwrap_or_else(|p| p.into_inner());
        init_devnet_with_mesa_blobs();
        let provers = WorkProvers::make(Some(TransactionVerifier::make())).unwrap();
        load_zkapp_provers(&provers);
        for job in [DEVNET_ZKAPP_SIGNED, DEVNET_ZKAPP_PROVED] {
            let zkapp = base_spec(job);
            let SingleSpec::Transition(statement, _) = &zkapp else {
                unreachable!()
            };
            let statement = statement.clone();
            let work = prove_check_and_gossip_with(WorkSpec::One(zkapp), &provers);
            assert_eq!(
                work.statement(),
                v2::TransactionSnarkWorkStatementStableV2::One(statement)
            );
        }
    }
}
