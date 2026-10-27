//! A bounded, versioned byte bridge from the block producer to a prover that
//! owns a separate memory (I34c).
//!
//! The web node's linear memory is capped at 4 GiB, and a Mesa block proof
//! peaks at about 2.5 GiB of prover memory (I34b). The web node therefore
//! proves in a second WASM instance with its own memory, in a hidden iframe,
//! and exchanges only owned bytes with it. This module is the node side of
//! that exchange, independent of the browser so that it is tested natively:
//!
//! - **requests** are a versioned binprot [`ProveRequest`] holding the
//!   block's prover input, its hash, its slot deadline and whether it is the
//!   genesis transition. No key is in the request: the producer key is
//!   installed once in each prover instance ([`ProverKey`]) and the prover
//!   substitutes it, or the public genesis winner's key for the genesis
//!   transition, as the native prover does in its own thread;
//! - **responses** are a versioned binprot [`ProveResponse`] that must echo
//!   the request ID and block hash, and come from the instance (generation)
//!   the request was sent to. Anything else is a stale answer and is ignored;
//! - **limits**: requests above [`MAX_REQUEST_BYTES`] are rejected before
//!   anything is copied out of the node, responses above
//!   [`MAX_RESPONSE_BYTES`] before decoding, and both decode exactly, without
//!   trailing bytes;
//! - **at most one active and one queued job**. A newer request replaces a
//!   queued one, which is rejected;
//! - **deadlines**: an active job that misses its slot is rejected and its
//!   prover instance retired, because a synchronous proof cannot be
//!   interrupted any other way. A queued job past its deadline is rejected;
//! - **failure**: when the instance dies or is retired, every pending job is
//!   rejected at once and a new instance is started, at most
//!   [`MAX_CONSECUTIVE_HOST_FAILURES`] times in a row. There is never a
//!   fallback to proving in the node's own memory.
//!
//! Every returned proof is verified against the expected protocol state with
//! the node's block verifier key before the node sees it
//! ([`verify_returned_proof`]); the producer state machine (P15) verifies it
//! once more before injecting the block.
//!
//! A node that runs a SNARK worker and no block producer proves its **SNARK
//! work** (P5.3) the same way: a [`WorkRequest`] carries one or two jobs and
//! the fee and public key their proofs commit to, and the bridge checks the
//! returned proofs as an OCaml snark pool would, with the node's transaction
//! verifier key and its own fee and key, before the worker state machine sees
//! them (`snark_worker::check_proved_work`).
//!
//! **A bridge serves one [`ProverRole`]**, never both. An instance proves one
//! thing at a time and cannot stop a proof it has started, short of being
//! retired, and a replacement takes one to two minutes to load its circuits:
//! a block job submitted while a SNARK proof runs would wait for that proof,
//! up to [`WORK_TIMEOUT_MS`], and miss its 90 s slot. Knowing our won slots
//! ahead cannot prevent that, because a won slot can become known late
//! (after bootstrap or at an epoch change). So a block producer's bridge
//! refuses the SNARK worker, and a worker's bridge refuses block jobs: a block
//! job never has SNARK work in front of it.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use ledger::proofs::{
    snark_worker::{spec_jobs, JobKind},
    verifiers::BlockVerifier,
};
use mina_node::{
    account::AccountSecretKey,
    block_producer::BlockProducerEvent,
    core::{channels::mpsc, constants::constraint_constants, thread},
    event_source::ExternalSnarkWorkerEvent,
    external_snark_worker::{
        ExternalSnarkWorkerError, ExternalSnarkWorkerWorkError, SnarkWorkSpec,
    },
    snark::{TransactionVerifier, VerifierSRS},
};
use mina_p2p_messages::{
    binprot::{self, BinProtRead, BinProtWrite},
    v2,
};

use crate::EventSender;

/// Version of the request, response and key messages below. A prover built
/// from a different version must refuse them.
pub const BRIDGE_VERSION: u32 = 1;

/// Upper bound on an encoded [`ProveRequest`]. Measured inputs: the Mesa
/// genesis transition is 22,053 bytes, a block that emits a ledger proof
/// 34,129 bytes (P4.6a's 548415 fixture). The bound leaves room for larger
/// stake-ledger witnesses while bounding the copy into the prover.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Upper bound on an encoded [`ProveResponse`]. A wrapped block proof is
/// 11,138 bytes.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// I34c runs measured up to 37.2 s per proof on this machine (31.5 s for
/// non-genesis). Round up to 40 s and reserve 5 s for verification/delivery.
/// Configure both for slower machines before dispatching work.
#[derive(Clone, Copy, Debug)]
pub struct ProofBudget {
    pub proof_ms: u64,
    pub margin_ms: u64,
}

impl Default for ProofBudget {
    fn default() -> Self {
        Self {
            proof_ms: 40_000,
            margin_ms: 5_000,
        }
    }
}

impl ProofBudget {
    pub fn minimum_remaining_ms(self) -> u64 {
        self.proof_ms.saturating_add(self.margin_ms).max(1)
    }
}

/// What a bridge and its prover instances prove: blocks or SNARK work, never
/// both (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProverRole {
    BlockProducer,
    SnarkWorker,
}

/// Why a block producer's prover takes no SNARK work, for the worker's error
/// and the log.
pub const WORKER_NEEDS_OWN_PROVER: &str = "the SNARK worker cannot share the block \
    producer's prover: a SNARK proof cannot be stopped for a block proof, which could \
    then miss its slot; run the SNARK worker in a node that does not produce blocks";

/// After this many prover instances in a row fail before or while proving,
/// stop starting new ones; requests are then rejected at once.
pub const MAX_CONSECUTIVE_HOST_FAILURES: u32 = 3;

#[derive(binprot::macros::BinProtRead, binprot::macros::BinProtWrite, Clone, Debug)]
pub struct ProveRequest {
    pub version: u32,
    pub request_id: u64,
    pub block_hash: v2::StateHash,
    /// Wall-clock milliseconds since the Unix epoch.
    pub deadline_ms: u64,
    /// The prover proves the genesis transition with the public genesis
    /// winner's key (`AccountSecretKey::genesis_producer`), as the node
    /// builds it, and any other block with the installed producer key.
    pub is_genesis: bool,
    /// `prover_state.producer_private_key` is a placeholder.
    pub input: v2::ProverExtendBlockchainInputStableV2,
}

#[derive(binprot::macros::BinProtRead, binprot::macros::BinProtWrite, Clone, Debug)]
pub struct ProveResponse {
    pub version: u32,
    pub request_id: u64,
    pub block_hash: v2::StateHash,
    pub proof: v2::MinaBaseProofStableV2,
}

/// Installed once in each prover instance, before its first request.
#[derive(binprot::macros::BinProtRead, binprot::macros::BinProtWrite, Clone)]
pub struct ProverKey {
    pub version: u32,
    pub key: v2::SignatureLibPrivateKeyStableV1,
}

impl ProverKey {
    pub fn new(keypair: &AccountSecretKey) -> Self {
        Self {
            version: BRIDGE_VERSION,
            key: keypair.into(),
        }
    }
}

/// SNARK work for the prover (P5.3): one or two jobs, proved for the sok
/// message of `fee` and `prover`, as OCaml's worker proves a spec for the
/// fee and public key its daemon was started with. Public data only: a
/// SNARK worker needs no private key.
#[derive(binprot::macros::BinProtRead, binprot::macros::BinProtWrite, Clone, Debug)]
pub struct WorkRequest {
    pub version: u32,
    pub request_id: u64,
    pub fee: v2::CurrencyFeeStableV1,
    pub prover: v2::NonZeroCurvePoint,
    pub spec: SnarkWorkSpec,
}

#[derive(binprot::macros::BinProtRead, binprot::macros::BinProtWrite, Clone, Debug)]
pub struct WorkResponse {
    pub version: u32,
    pub request_id: u64,
    pub proofs: v2::TransactionSnarkWorkTStableV2Proofs,
    /// Each job's time in the prover (witness, step and wrap), in
    /// milliseconds, in the spec's order.
    pub prove_ms: Vec<u64>,
}

/// A work job that runs this long has hung its instance, which is retired.
/// OCaml reassigns a job after 420 s (`--work-reassignment-wait`); one
/// browser base proof took 15 to 34 s (I34).
pub const WORK_TIMEOUT_MS: u64 = 600_000;

/// The statements the spec's jobs prove, in its order.
pub fn work_statements(spec: &SnarkWorkSpec) -> Vec<v2::MinaStateSnarkedLedgerStateStableV2> {
    use v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single as Single;
    let statement = |single: &Single| match single {
        Single::Transition(statement, _) => statement.clone(),
        Single::Merge(merge) => merge.0.clone(),
    };
    match spec {
        SnarkWorkSpec::One(one) => vec![statement(one)],
        SnarkWorkSpec::Two((one, two)) => vec![statement(one), statement(two)],
    }
}

/// `base`, `merge`, `base+zkapp/2`, ... for the log: a zkApp command's
/// base job with its segment count.
pub fn work_kinds(spec: &SnarkWorkSpec) -> String {
    spec_jobs(spec)
        .into_iter()
        .map(|single| JobKind::of(single).to_string())
        .collect::<Vec<_>>()
        .join("+")
}

pub fn encode(value: &impl BinProtWrite) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    value
        .binprot_write(&mut bytes)
        .map_err(|error| format!("prover message unavailable: {error}"))?;
    Ok(bytes)
}

/// Decodes exactly one value from at most `limit` bytes.
pub fn decode_exact<T: BinProtRead>(bytes: &[u8], limit: usize) -> Result<T, String> {
    if bytes.len() > limit {
        return Err(format!(
            "message of {} bytes exceeds the {limit}-byte limit",
            bytes.len()
        ));
    }
    let mut remaining = bytes;
    let value = T::binprot_read(&mut remaining).map_err(|error| format!("decode: {error}"))?;
    if !remaining.is_empty() {
        return Err(format!("{} trailing bytes", remaining.len()));
    }
    Ok(value)
}

/// Same rule as the native prover: the genesis transition is proved with
/// the key in its input, every other block with the producer's key.
pub fn is_genesis_input(input: &v2::ProverExtendBlockchainInputStableV2) -> bool {
    let height = input
        .next_state
        .body
        .consensus_state
        .blockchain_length
        .as_u32();
    height == 1
        || constraint_constants()
            .fork
            .as_ref()
            .is_some_and(|fork| fork.blockchain_length + 1 == height)
}

/// When this block can no longer be produced: the end of its slot. The
/// block's timestamp is its slot's start.
pub fn slot_deadline_ms(input: &v2::ProverExtendBlockchainInputStableV2) -> u64 {
    input
        .next_state
        .body
        .blockchain_state
        .timestamp
        .as_u64()
        .saturating_add(constraint_constants().block_window_duration_ms)
}

/// The node's block verifier key and its verifier SRS. The SRS is cached
/// per thread, so the node passes its own instead of the bridge thread
/// building another.
#[derive(Clone)]
pub struct BlockProofChecker {
    pub verifier: BlockVerifier,
    pub srs: Arc<VerifierSRS>,
}

/// The check peers make: the proof verifies against the expected protocol
/// state, the one the node asked to be proved, with the node's key.
pub fn verify_returned_proof(
    expected_state: &v2::MinaStateProtocolStateValueStableV2,
    proof: &Arc<v2::MinaBaseProofStableV2>,
    checker: &BlockProofChecker,
) -> bool {
    let header = v2::MinaBlockHeaderStableV2 {
        protocol_state: expected_state.clone(),
        protocol_state_proof: proof.clone(),
        delta_block_chain_proof: (
            expected_state.previous_state_hash.clone(),
            std::iter::empty().collect(),
        ),
        current_protocol_version: mina_node::core::constants::PROTOCOL_VERSION.clone(),
        proposed_protocol_version_opt: None,
    };
    ledger::proofs::verification::verify_block(&header, &checker.verifier, &checker.srs)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostState {
    /// No prover instance was started yet.
    Down,
    Starting(u64),
    Ready(u64),
    /// Too many instances failed in a row; production can't prove.
    Disabled(String),
}

/// What the transport must do with the prover instances.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Create prover instance `generation`, load its circuits and install
    /// the producer key. Report `HostReady` or `HostFailed`.
    Start { generation: u64 },
    /// Send an encoded [`ProveRequest`] to instance `generation`. Report its
    /// `Response`, or `ProveError` if the prover refused it, or `HostFailed`.
    Prove {
        generation: u64,
        request_id: u64,
        deadline_ms: u64,
        is_genesis: bool,
        bytes: Vec<u8>,
    },
    /// Send an encoded [`WorkRequest`] to instance `generation`. Report its
    /// `Response`, or `ProveError` if the prover refused it, or `HostFailed`;
    /// fail the instance if it has not answered within `timeout_ms`.
    ProveWork {
        generation: u64,
        request_id: u64,
        timeout_ms: u64,
        bytes: Vec<u8>,
    },
    /// Terminate every worker of instance `generation` and remove it.
    Retire { generation: u64, reason: String },
}

/// What the SNARK worker proves for, and checks its proofs with before it
/// offers them: the network's transaction verifier key and its SRS, and
/// the fee and public key the proofs must commit to (P5.2's self-check).
#[derive(Clone)]
pub struct WorkChecker {
    pub fee: v2::CurrencyFeeStableV1,
    pub prover: v2::NonZeroCurvePoint,
    pub verifier: TransactionVerifier,
    pub srs: Arc<VerifierSRS>,
}

/// What the node must be told.
#[derive(Debug)]
pub enum Outcome {
    /// The active job's prover answered: verify before announcing.
    Returned {
        block_hash: v2::StateHash,
        request_id: u64,
        expected_state: Box<v2::MinaStateProtocolStateValueStableV2>,
        proof: Arc<v2::MinaBaseProofStableV2>,
        /// From dispatch to response, in milliseconds.
        prove_ms: u64,
        deadline_ms: u64,
    },
    Rejected {
        block_hash: v2::StateHash,
        request_id: u64,
        error: String,
    },
    /// The active work job's prover answered: check before offering.
    WorkReturned {
        request_id: u64,
        kinds: String,
        /// The jobs' statements, from the spec the node asked for.
        statements: Vec<v2::MinaStateSnarkedLedgerStateStableV2>,
        proofs: v2::TransactionSnarkWorkTStableV2Proofs,
        /// Each job's time in the prover.
        prove_ms: Vec<u64>,
        /// From dispatch to response.
        round_trip_ms: u64,
        /// From submission to dispatch: the time it waited for the instance.
        queued_ms: u64,
        checker: Arc<WorkChecker>,
    },
    WorkRejected {
        request_id: u64,
        error: String,
    },
    /// A queued work job was dropped on the worker's request, unproved.
    WorkCancelled {
        request_id: u64,
    },
    WorkerStarted,
    WorkerKilled,
    /// The worker cannot prove: no prover instance can be started.
    WorkerFailed {
        error: String,
    },
}

#[derive(Default, Debug)]
pub struct Step {
    pub commands: Vec<Command>,
    pub outcomes: Vec<Outcome>,
    /// Ignored stale messages, for the log.
    pub ignored: Vec<String>,
}

struct Job {
    request_id: u64,
    block_hash: v2::StateHash,
    deadline_ms: u64,
    is_genesis: bool,
    bytes: Vec<u8>,
    expected_state: Box<v2::MinaStateProtocolStateValueStableV2>,
    /// The instance and time it was sent to, once sent.
    dispatched: Option<(u64, u64)>,
    /// When it was submitted, to log a wait behind a work job.
    submitted_ms: u64,
}

struct WorkJob {
    request_id: u64,
    kinds: String,
    statements: Vec<v2::MinaStateSnarkedLedgerStateStableV2>,
    bytes: Vec<u8>,
    checker: Arc<WorkChecker>,
    submitted_ms: u64,
    /// The instance and time it was sent to, once sent.
    dispatched: Option<(u64, u64)>,
    /// The worker was killed while this was proved: drop its answer.
    orphaned: bool,
}

impl std::fmt::Debug for WorkChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkChecker")
            .field("fee", &self.fee)
            .field("prover", &self.prover)
            .finish_non_exhaustive()
    }
}

/// The node side of the bridge, as a state machine without I/O.
pub struct BridgeCore {
    role: ProverRole,
    host: HostState,
    last_generation: u64,
    consecutive_failures: u32,
    next_request_id: u64,
    proof_budget: ProofBudget,
    active: Option<Job>,
    queued: Option<Job>,
    /// The SNARK worker, while it runs; only in a [`ProverRole::SnarkWorker`]
    /// bridge, which has no block jobs.
    worker: Option<Arc<WorkChecker>>,
    /// At most one work job is in the bridge: the worker state machine
    /// submits the next only after this one's outcome.
    work_active: Option<WorkJob>,
    work_queued: Option<WorkJob>,
}

impl BridgeCore {
    pub fn new(role: ProverRole) -> Self {
        Self {
            role,
            host: HostState::Down,
            last_generation: 0,
            consecutive_failures: 0,
            next_request_id: 0,
            proof_budget: ProofBudget::default(),
            active: None,
            queued: None,
            worker: None,
            work_active: None,
            work_queued: None,
        }
    }

    pub fn host(&self) -> &HostState {
        &self.host
    }

    pub fn role(&self) -> ProverRole {
        self.role
    }

    /// Block jobs in the bridge.
    pub fn pending(&self) -> usize {
        usize::from(self.active.is_some()) + usize::from(self.queued.is_some())
    }

    /// Work jobs in the bridge.
    pub fn pending_work(&self) -> usize {
        usize::from(self.work_active.is_some()) + usize::from(self.work_queued.is_some())
    }

    /// Prewarm: start the first instance before any slot is won.
    pub fn start(&mut self) -> Step {
        let mut step = Step::default();
        if matches!(self.host, HostState::Down) {
            self.start_next(&mut step);
        }
        step
    }

    fn start_next(&mut self, step: &mut Step) {
        self.last_generation += 1;
        self.host = HostState::Starting(self.last_generation);
        step.commands.push(Command::Start {
            generation: self.last_generation,
        });
    }

    fn active_matches(&self, generation: u64, request_id: u64) -> bool {
        self.active.as_ref().is_some_and(|job| {
            job.request_id == request_id && job.dispatched.map(|d| d.0) == Some(generation)
        })
    }

    fn work_active_matches(&self, generation: u64, request_id: u64) -> bool {
        self.work_active.as_ref().is_some_and(|job| {
            job.request_id == request_id && job.dispatched.map(|d| d.0) == Some(generation)
        })
    }

    /// The SNARK worker starts proving with `checker`'s fee and key, unless
    /// this bridge proves blocks.
    pub fn start_worker(&mut self, checker: WorkChecker) -> Step {
        let mut step = Step::default();
        if self.role == ProverRole::BlockProducer {
            step.outcomes.push(Outcome::WorkerFailed {
                error: WORKER_NEEDS_OWN_PROVER.to_owned(),
            });
            return step;
        }
        match &self.host {
            HostState::Disabled(reason) => step.outcomes.push(Outcome::WorkerFailed {
                error: format!("isolated prover disabled: {reason}"),
            }),
            HostState::Down => step.outcomes.push(Outcome::WorkerFailed {
                error: "isolated prover was not started".to_owned(),
            }),
            _ => {
                self.worker = Some(Arc::new(checker));
                step.outcomes.push(Outcome::WorkerStarted);
            }
        }
        step
    }

    /// Queues one work bundle; it is sent when the instance is ready and
    /// idle. A worker that is not running gets a job error.
    pub fn submit_work(&mut self, spec: SnarkWorkSpec, now_ms: u64) -> Step {
        let mut step = Step::default();
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        let Some(checker) = self.worker.clone() else {
            step.outcomes.push(Outcome::WorkRejected {
                request_id,
                error: "the SNARK worker is not running".to_owned(),
            });
            return step;
        };
        if let HostState::Disabled(reason) = &self.host {
            step.outcomes.push(Outcome::WorkerFailed {
                error: format!("isolated prover disabled: {reason}"),
            });
            return step;
        }
        let kinds = work_kinds(&spec);
        let statements = work_statements(&spec);
        let bytes = match encode(&WorkRequest {
            version: BRIDGE_VERSION,
            request_id,
            fee: checker.fee.clone(),
            prover: checker.prover.clone(),
            spec,
        }) {
            Ok(bytes) => bytes,
            Err(error) => {
                step.outcomes
                    .push(Outcome::WorkRejected { request_id, error });
                return step;
            }
        };
        if bytes.len() > MAX_REQUEST_BYTES {
            step.outcomes.push(Outcome::WorkRejected {
                request_id,
                error: format!(
                    "SNARK work request of {} bytes exceeds the {MAX_REQUEST_BYTES}-byte limit",
                    bytes.len()
                ),
            });
            return step;
        }
        let job = WorkJob {
            request_id,
            kinds,
            statements,
            bytes,
            checker,
            submitted_ms: now_ms,
            dispatched: None,
            orphaned: false,
        };
        if let Some(old) = self.work_queued.replace(job) {
            // The worker submits one job at a time; keep the newest.
            step.ignored.push(format!(
                "work request {} replaced while queued",
                old.request_id
            ));
        }
        self.dispatch(now_ms, &mut step);
        step
    }

    /// The worker gave its job up. A queued job is dropped now; one being
    /// proved cannot be stopped, and its proof is still offered, as the
    /// in-process worker's is (P5.2).
    pub fn cancel_work(&mut self) -> Step {
        let mut step = Step::default();
        if let Some(job) = self.work_queued.take() {
            step.outcomes.push(Outcome::WorkCancelled {
                request_id: job.request_id,
            });
        } else if self.work_active.is_none() {
            step.outcomes.push(Outcome::WorkCancelled { request_id: 0 });
        }
        step
    }

    /// The worker stops: a queued job is dropped, the answer to one being
    /// proved will be.
    pub fn kill_worker(&mut self) -> Step {
        let mut step = Step::default();
        self.worker = None;
        self.work_queued = None;
        if let Some(job) = &mut self.work_active {
            job.orphaned = true;
        }
        step.outcomes.push(Outcome::WorkerKilled);
        step
    }

    pub fn submit(
        &mut self,
        block_hash: v2::StateHash,
        mut input: v2::ProverExtendBlockchainInputStableV2,
        is_genesis: bool,
        deadline_ms: u64,
        now_ms: u64,
    ) -> Step {
        let mut step = Step::default();
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        let refusal = match &self.host {
            _ if self.role == ProverRole::SnarkWorker => {
                Some("this prover proves SNARK work only".to_owned())
            }
            HostState::Disabled(reason) => {
                Some(format!("isolated block prover disabled: {reason}"))
            }
            HostState::Down => Some("isolated block prover was not started".to_owned()),
            _ if deadline_ms <= now_ms => Some("block proof deadline already passed".to_owned()),
            _ => None,
        };
        if let Some(error) = refusal {
            step.outcomes.push(Outcome::Rejected {
                block_hash,
                request_id,
                error,
            });
            return step;
        }
        // The key is installed in the prover; never copy it into requests.
        input.prover_state.producer_private_key =
            v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one());
        let expected_state = Box::new(input.next_state.clone());
        let bytes = match encode(&ProveRequest {
            version: BRIDGE_VERSION,
            request_id,
            block_hash: block_hash.clone(),
            deadline_ms,
            is_genesis,
            input,
        }) {
            Ok(bytes) => bytes,
            Err(error) => {
                step.outcomes.push(Outcome::Rejected {
                    block_hash,
                    request_id,
                    error,
                });
                return step;
            }
        };
        if bytes.len() > MAX_REQUEST_BYTES {
            step.outcomes.push(Outcome::Rejected {
                block_hash,
                request_id,
                error: format!(
                    "block prover request of {} bytes exceeds the {MAX_REQUEST_BYTES}-byte limit",
                    bytes.len()
                ),
            });
            return step;
        }
        let job = Job {
            request_id,
            block_hash,
            deadline_ms,
            is_genesis,
            bytes,
            expected_state,
            dispatched: None,
            submitted_ms: now_ms,
        };
        if let Some(old) = self.queued.replace(job) {
            step.outcomes.push(Outcome::Rejected {
                block_hash: old.block_hash,
                request_id: old.request_id,
                error: "superseded by a newer block proof request".to_owned(),
            });
        }
        self.dispatch(now_ms, &mut step);
        step
    }

    /// Sends the next job if the instance is ready and idle. A bridge has
    /// block jobs or work jobs, never both ([`ProverRole`]).
    fn dispatch(&mut self, now_ms: u64, step: &mut Step) {
        debug_assert!(
            (self.active.is_none() && self.queued.is_none())
                || (self.work_active.is_none() && self.work_queued.is_none()),
            "block and work jobs in one bridge"
        );
        let HostState::Ready(generation) = self.host else {
            return;
        };
        if self.active.is_some() || self.work_active.is_some() {
            return;
        }
        if let Some(mut job) = self.queued.take() {
            let remaining = job.deadline_ms.saturating_sub(now_ms);
            let required = self.proof_budget.minimum_remaining_ms();
            if remaining < required {
                step.outcomes.push(Outcome::Rejected {
                    block_hash: job.block_hash,
                    request_id: job.request_id,
                    error: format!("not enough time before block proof deadline: {remaining} ms left, {required} ms required"),
                });
                // No work was sent: keep the healthy instance and its failure
                // budget.
            } else {
                let waited = now_ms.saturating_sub(job.submitted_ms);
                if waited > 0 {
                    // The instance was starting, or proving the block job
                    // this one superseded; never SNARK work (P5.3).
                    mina_core::warn!(
                        message = "block proof waited for the prover",
                        request_id = job.request_id,
                        waited_ms = waited
                    );
                }
                job.dispatched = Some((generation, now_ms));
                step.commands.push(Command::Prove {
                    generation,
                    request_id: job.request_id,
                    deadline_ms: job.deadline_ms,
                    is_genesis: job.is_genesis,
                    // The transport copies these out; the job keeps no second
                    // copy.
                    bytes: std::mem::take(&mut job.bytes),
                });
                self.active = Some(job);
                return;
            }
        }
        if let Some(mut job) = self.work_queued.take() {
            job.dispatched = Some((generation, now_ms));
            step.commands.push(Command::ProveWork {
                generation,
                request_id: job.request_id,
                timeout_ms: WORK_TIMEOUT_MS,
                bytes: std::mem::take(&mut job.bytes),
            });
            self.work_active = Some(job);
        }
    }

    pub fn host_ready(&mut self, generation: u64, now_ms: u64) -> Step {
        let mut step = Step::default();
        if self.host != HostState::Starting(generation) {
            step.ignored
                .push(format!("ready from stale prover instance {generation}"));
            return step;
        }
        self.host = HostState::Ready(generation);
        self.dispatch(now_ms, &mut step);
        step
    }

    /// The instance died, failed to start, or was retired: reject every
    /// pending job now, and start a replacement for later slots.
    pub fn host_failed(&mut self, generation: u64, reason: &str) -> Step {
        let mut step = Step::default();
        let current = match self.host {
            HostState::Starting(g) | HostState::Ready(g) => g == generation,
            _ => false,
        };
        if !current {
            step.ignored.push(format!(
                "failure of stale prover instance {generation}: {reason}"
            ));
            return step;
        }
        self.fail_current(generation, reason, &mut step);
        step
    }

    fn fail_current(&mut self, generation: u64, reason: &str, step: &mut Step) {
        let error = format!("isolated block prover instance {generation} failed: {reason}");
        for job in [self.active.take(), self.queued.take()]
            .into_iter()
            .flatten()
        {
            step.outcomes.push(Outcome::Rejected {
                block_hash: job.block_hash,
                request_id: job.request_id,
                error: error.clone(),
            });
        }
        // The work job being proved is lost; a queued one waits for the
        // replacement.
        if let Some(job) = self.work_active.take().filter(|job| !job.orphaned) {
            step.outcomes.push(Outcome::WorkRejected {
                request_id: job.request_id,
                error: error.clone(),
            });
        }
        step.commands.push(Command::Retire {
            generation,
            reason: reason.to_owned(),
        });
        self.consecutive_failures += 1;
        if self.consecutive_failures >= MAX_CONSECUTIVE_HOST_FAILURES {
            let reason = format!(
                "{} prover instances failed in a row, the last: {reason}",
                self.consecutive_failures
            );
            if self.worker.is_some() {
                // The worker stops for good: a job error would only make it
                // take the next job.
                self.work_queued = None;
                step.outcomes.push(Outcome::WorkerFailed {
                    error: format!("isolated prover disabled: {reason}"),
                });
            }
            self.host = HostState::Disabled(reason);
        } else {
            self.start_next(step);
        }
    }

    pub fn response(
        &mut self,
        generation: u64,
        request_id: u64,
        bytes: &[u8],
        now_ms: u64,
    ) -> Step {
        let mut step = Step::default();
        if self.work_active_matches(generation, request_id) {
            self.work_response(generation, bytes, now_ms, &mut step);
            return step;
        }
        if !self.active_matches(generation, request_id) {
            step.ignored.push(format!(
                "stale response to request {request_id} from prover instance {generation}"
            ));
            return step;
        }
        let job = self.active.take().expect("checked above");
        if now_ms >= job.deadline_ms {
            step.outcomes.push(Outcome::Rejected {
                block_hash: job.block_hash,
                request_id: job.request_id,
                error: "block proof response arrived after its deadline".to_owned(),
            });
            // The proof has finished; the host need not be retired.
            self.dispatch(now_ms, &mut step);
            return step;
        }
        let response = decode_exact::<ProveResponse>(bytes, MAX_RESPONSE_BYTES).and_then(|r| {
            if r.version != BRIDGE_VERSION {
                Err(format!(
                    "response version {} is not {BRIDGE_VERSION}",
                    r.version
                ))
            } else if r.request_id != job.request_id || r.block_hash != job.block_hash {
                Err("response names a different request or block".to_owned())
            } else {
                Ok(r)
            }
        });
        match response {
            Ok(response) => {
                // A good answer resets the instance failure budget.
                self.consecutive_failures = 0;
                let started = job.dispatched.map_or(now_ms, |d| d.1);
                step.outcomes.push(Outcome::Returned {
                    block_hash: job.block_hash,
                    request_id: job.request_id,
                    expected_state: job.expected_state,
                    proof: Arc::new(response.proof),
                    prove_ms: now_ms.saturating_sub(started),
                    deadline_ms: job.deadline_ms,
                });
                self.dispatch(now_ms, &mut step);
            }
            Err(error) => {
                // The prover broke the protocol: trust nothing else from it.
                step.outcomes.push(Outcome::Rejected {
                    block_hash: job.block_hash,
                    request_id: job.request_id,
                    error: format!("invalid block prover response: {error}"),
                });
                self.fail_current(generation, "invalid response", &mut step);
            }
        }
        step
    }

    /// The work job's answer: its proofs, one per job, for the checks the
    /// node makes before offering them.
    fn work_response(&mut self, generation: u64, bytes: &[u8], now_ms: u64, step: &mut Step) {
        let job = self.work_active.take().expect("checked by the caller");
        let jobs = job.statements.len();
        let response = decode_exact::<WorkResponse>(bytes, MAX_RESPONSE_BYTES).and_then(|r| {
            let proofs = match &r.proofs {
                v2::TransactionSnarkWorkTStableV2Proofs::One(_) => 1,
                v2::TransactionSnarkWorkTStableV2Proofs::Two(_) => 2,
            };
            if r.version != BRIDGE_VERSION {
                Err(format!(
                    "response version {} is not {BRIDGE_VERSION}",
                    r.version
                ))
            } else if r.request_id != job.request_id {
                Err("response names a different request".to_owned())
            } else if proofs != jobs || r.prove_ms.len() != jobs {
                Err(format!("{proofs} proofs for {jobs} jobs"))
            } else {
                Ok(r)
            }
        });
        match response {
            Ok(response) => {
                // A good answer resets the instance failure budget.
                self.consecutive_failures = 0;
                let (instance, started) = job.dispatched.unwrap_or((generation, now_ms));
                debug_assert_eq!(instance, generation);
                if !job.orphaned {
                    step.outcomes.push(Outcome::WorkReturned {
                        request_id: job.request_id,
                        kinds: job.kinds,
                        statements: job.statements,
                        proofs: response.proofs,
                        prove_ms: response.prove_ms,
                        round_trip_ms: now_ms.saturating_sub(started),
                        queued_ms: started.saturating_sub(job.submitted_ms),
                        checker: job.checker,
                    });
                }
                self.dispatch(now_ms, step);
            }
            Err(error) => {
                // The prover broke the protocol: trust nothing else from it.
                if !job.orphaned {
                    step.outcomes.push(Outcome::WorkRejected {
                        request_id: job.request_id,
                        error: format!("invalid SNARK work response: {error}"),
                    });
                }
                self.fail_current(generation, "invalid response", step);
            }
        }
    }

    /// The prover refused or failed the request but stays usable, e.g. a
    /// witness that does not satisfy the circuit.
    pub fn prove_error(
        &mut self,
        generation: u64,
        request_id: u64,
        error: &str,
        now_ms: u64,
    ) -> Step {
        let mut step = Step::default();
        if self.work_active_matches(generation, request_id) {
            let job = self.work_active.take().expect("checked above");
            if !job.orphaned {
                step.outcomes.push(Outcome::WorkRejected {
                    request_id: job.request_id,
                    error: format!("SNARK work prover: {error}"),
                });
            }
            self.dispatch(now_ms, &mut step);
            return step;
        }
        if !self.active_matches(generation, request_id) {
            step.ignored.push(format!(
                "stale error for request {request_id} from prover instance {generation}: {error}"
            ));
            return step;
        }
        let job = self.active.take().expect("checked above");
        step.outcomes.push(Outcome::Rejected {
            block_hash: job.block_hash,
            request_id: job.request_id,
            error: format!("block prover: {error}"),
        });
        self.dispatch(now_ms, &mut step);
        step
    }

    /// Enforces deadlines. Call about once a second.
    pub fn tick(&mut self, now_ms: u64) -> Step {
        let mut step = Step::default();
        if self
            .queued
            .as_ref()
            .is_some_and(|job| job.deadline_ms <= now_ms)
        {
            let job = self.queued.take().expect("checked above");
            step.outcomes.push(Outcome::Rejected {
                block_hash: job.block_hash,
                request_id: job.request_id,
                error: "block proof deadline passed while queued".to_owned(),
            });
        }
        let expired = self
            .active
            .as_ref()
            .filter(|job| job.deadline_ms <= now_ms)
            .and_then(|job| job.dispatched.map(|d| d.0));
        if let Some(generation) = expired {
            // Proving cannot be interrupted: retire the whole instance.
            self.fail_current(generation, "block proof missed its deadline", &mut step);
        }
        let hung = self
            .work_active
            .as_ref()
            .and_then(|job| job.dispatched)
            .filter(|(_, sent)| now_ms.saturating_sub(*sent) >= WORK_TIMEOUT_MS);
        if let Some((generation, _)) = hung {
            self.fail_current(generation, "SNARK work proof timed out", &mut step);
        }
        step
    }
}

/// What reaches the bridge thread, from the node or from the transport.
pub enum BridgeInput {
    /// Set the measured proof duration and delivery/verification reserve.
    SetProofBudget(ProofBudget),
    Submit {
        block_hash: v2::StateHash,
        input: Box<v2::ProverExtendBlockchainInputStableV2>,
        deadline_ms: u64,
    },
    /// The SNARK worker starts: what its proofs commit to and are checked
    /// with.
    StartWorker(Box<WorkChecker>),
    SubmitWork(Box<SnarkWorkSpec>),
    CancelWork,
    KillWorker,
    HostReady {
        generation: u64,
        /// The instance's setup report (JSON), for the log.
        report: String,
    },
    HostFailed {
        generation: u64,
        reason: String,
    },
    Response {
        generation: u64,
        request_id: u64,
        bytes: Vec<u8>,
    },
    ProveError {
        generation: u64,
        request_id: u64,
        error: String,
    },
    /// Sent by the transport about once a second, for deadlines.
    Tick,
}

pub type BridgeInputSender = mpsc::UnboundedSender<BridgeInput>;

/// The node side of an isolated prover, held by the block producer service
/// in place of an in-memory `BlockProver`, or by the SNARK worker service in
/// place of its proving thread (P5.3), never by both.
#[derive(Clone)]
pub struct IsolatedBlockProver {
    input: BridgeInputSender,
    /// Submitted block jobs that have no outcome yet. Each job has exactly
    /// one.
    pending: Arc<AtomicUsize>,
    event_sender: EventSender,
}

impl IsolatedBlockProver {
    /// Starts the bridge thread, which starts the first prover instance at
    /// once (the prewarm). `transport` carries each [`Command`] to the
    /// prover instances; their answers, and a tick about once a second, come
    /// back through the returned sender. `checker` verifies returned block
    /// proofs; a node that does not produce blocks has none, and its bridge
    /// proves SNARK work only ([`ProverRole::SnarkWorker`]). A block
    /// producer's bridge refuses the SNARK worker.
    pub fn start(
        event_sender: EventSender,
        checker: Option<BlockProofChecker>,
        transport: impl FnMut(Command) + Send + 'static,
    ) -> (Self, BridgeInputSender) {
        let (input, rx) = mpsc::unbounded_channel::<BridgeInput>();
        let pending = Arc::new(AtomicUsize::new(0));
        let loop_pending = pending.clone();
        let failure_event_sender = event_sender.clone();
        thread::Builder::new()
            .name("mina_block_prover".to_owned())
            .spawn(move || bridge_loop(rx, event_sender, checker, loop_pending, transport))
            .expect("failed to spawn the block prover bridge thread");
        (
            Self {
                input: input.clone(),
                pending,
                event_sender: failure_event_sender,
            },
            input,
        )
    }

    pub fn prove(
        &self,
        block_hash: v2::StateHash,
        input: Box<v2::ProverExtendBlockchainInputStableV2>,
        deadline_ms: u64,
    ) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        if self
            .input
            .send(BridgeInput::Submit {
                block_hash: block_hash.clone(),
                input,
                deadline_ms,
            })
            .is_err()
        {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            let error = "block prover bridge thread is gone".to_owned();
            let _ = self
                .event_sender
                .send(BlockProducerEvent::BlockProve(block_hash, Err(error)).into());
        }
    }

    pub fn pending_requests(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// Sends a SNARK worker command to the bridge thread. `false` if the
    /// thread is gone.
    pub fn worker_input(&self, input: BridgeInput) -> bool {
        self.input.send(input).is_ok()
    }
}

/// Wall-clock milliseconds since the Unix epoch, the unit of the deadlines.
///
/// Not `redux::Timestamp::global_now()`: on wasm32 it measures from a
/// monotonic instant taken by the first thread that asked, and every web
/// worker has its own monotonic clock, so in the bridge's worker it panics
/// (`ms >= 0.0` in wasm-timer).
pub fn now_ms() -> u64 {
    redux::SystemTime::now()
        .duration_since(redux::SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Feeds one input to the core, for the bridge thread and for tests.
pub fn handle_input(core: &mut BridgeCore, input: BridgeInput, now_ms: u64) -> Step {
    match input {
        BridgeInput::SetProofBudget(budget) => {
            core.proof_budget = budget;
            Step::default()
        }
        BridgeInput::Submit {
            block_hash,
            input,
            deadline_ms,
        } => {
            let is_genesis = is_genesis_input(&input);
            core.submit(block_hash, *input, is_genesis, deadline_ms, now_ms)
        }
        BridgeInput::HostReady { generation, report } => {
            mina_core::info!(
                message = "isolated block prover ready",
                generation = generation,
                report = report
            );
            core.host_ready(generation, now_ms)
        }
        BridgeInput::HostFailed { generation, reason } => {
            mina_core::warn!(
                message = "isolated block prover failed",
                generation = generation,
                reason = reason.clone()
            );
            core.host_failed(generation, &reason)
        }
        BridgeInput::Response {
            generation,
            request_id,
            bytes,
        } => core.response(generation, request_id, &bytes, now_ms),
        BridgeInput::ProveError {
            generation,
            request_id,
            error,
        } => core.prove_error(generation, request_id, &error, now_ms),
        BridgeInput::Tick => core.tick(now_ms),
        BridgeInput::StartWorker(checker) => core.start_worker(*checker),
        BridgeInput::SubmitWork(spec) => core.submit_work(*spec, now_ms),
        BridgeInput::CancelWork => core.cancel_work(),
        BridgeInput::KillWorker => core.kill_worker(),
    }
}

/// A block job's outcome, for the block producer.
pub enum BlockOutcome {
    Returned {
        block_hash: v2::StateHash,
        request_id: u64,
        expected_state: Box<v2::MinaStateProtocolStateValueStableV2>,
        proof: Arc<v2::MinaBaseProofStableV2>,
        prove_ms: u64,
        deadline_ms: u64,
    },
    Rejected {
        block_hash: v2::StateHash,
        request_id: u64,
        error: String,
    },
}

impl Outcome {
    /// A block job's outcome, or the worker's event for any other.
    pub fn split(self) -> Result<BlockOutcome, Outcome> {
        match self {
            Outcome::Returned {
                block_hash,
                request_id,
                expected_state,
                proof,
                prove_ms,
                deadline_ms,
            } => Ok(BlockOutcome::Returned {
                block_hash,
                request_id,
                expected_state,
                proof,
                prove_ms,
                deadline_ms,
            }),
            Outcome::Rejected {
                block_hash,
                request_id,
                error,
            } => Ok(BlockOutcome::Rejected {
                block_hash,
                request_id,
                error,
            }),
            other => Err(other),
        }
    }
}

/// Turns an outcome into the node's `BlockProve` result, verifying a
/// returned proof first. A node without a block verifier refuses every
/// returned proof (it never asks for one).
pub fn outcome_result(
    outcome: BlockOutcome,
    checker: Option<&BlockProofChecker>,
) -> (
    v2::StateHash,
    Result<Arc<v2::MinaBaseProofStableV2>, String>,
) {
    outcome_result_with(
        outcome,
        |state, proof| checker.is_some_and(|checker| verify_returned_proof(state, proof, checker)),
        now_ms,
    )
}

fn outcome_result_with(
    outcome: BlockOutcome,
    mut verify: impl FnMut(
        &v2::MinaStateProtocolStateValueStableV2,
        &Arc<v2::MinaBaseProofStableV2>,
    ) -> bool,
    mut clock: impl FnMut() -> u64,
) -> (
    v2::StateHash,
    Result<Arc<v2::MinaBaseProofStableV2>, String>,
) {
    match outcome {
        BlockOutcome::Rejected {
            block_hash,
            request_id,
            error,
        } => {
            mina_core::error!(
                message = "isolated block proof failed",
                block_hash = block_hash.to_string(),
                request_id = request_id,
                error = error.clone()
            );
            (block_hash, Err(error))
        }
        BlockOutcome::Returned {
            block_hash,
            request_id,
            expected_state,
            proof,
            prove_ms,
            deadline_ms,
        } => {
            let started = clock();
            if started >= deadline_ms {
                return (
                    block_hash,
                    Err("block proof deadline passed before verification".to_owned()),
                );
            }
            let verified = verify(&expected_state, &proof);
            let finished = clock();
            mina_core::info!(
                message = "isolated block proof returned",
                block_hash = block_hash.to_string(),
                request_id = request_id,
                prove_ms = prove_ms,
                verify_ms = finished.saturating_sub(started),
                verified = verified
            );
            let result = if finished >= deadline_ms {
                Err("block proof deadline passed during verification".to_owned())
            } else if verified {
                Ok(proof)
            } else {
                Err(
                    "the isolated prover's proof does not verify against the expected \
                     protocol state and the node's block verifier key"
                        .to_owned(),
                )
            };
            (block_hash, result)
        }
    }
}

/// Whether each returned proof is of the statement its job asked for (the
/// sok digest is checked with the proof, in [`work_event`]).
pub fn proofs_match_statements(
    proofs: &v2::TransactionSnarkWorkTStableV2Proofs,
    statements: &[v2::MinaStateSnarkedLedgerStateStableV2],
) -> bool {
    let proofs = match proofs {
        v2::TransactionSnarkWorkTStableV2Proofs::One(one) => vec![one],
        v2::TransactionSnarkWorkTStableV2Proofs::Two((one, two)) => vec![one, two],
    };
    proofs.len() == statements.len()
        && proofs.iter().zip(statements).all(|(proof, statement)| {
            let (proved, asked) = (&proof.0.statement, &statement.0);
            proved.source == asked.source
                && proved.target == asked.target
                && proved.connecting_ledger_left == asked.connecting_ledger_left
                && proved.connecting_ledger_right == asked.connecting_ledger_right
                && proved.supply_increase == asked.supply_increase
                && proved.fee_excess == asked.fee_excess
        })
}

/// The SNARK worker's event for a work outcome. Returned proofs are offered
/// only if they are of the statements asked for and pass P5.2's self-check
/// (`snark_worker::check_proved_work`): they verify with the network's
/// transaction key for the sok digest of our own fee and key.
pub fn work_event(outcome: Outcome) -> Option<ExternalSnarkWorkerEvent> {
    work_event_with(outcome, |work, times, checker| {
        crate::snark_worker::check_proved_work(work, times, &checker.verifier, &checker.srs)
    })
}

fn work_event_with(
    outcome: Outcome,
    check: impl FnOnce(
        mina_node::core::snark::Snark,
        crate::snark_worker::WorkTimes,
        &WorkChecker,
    ) -> Result<mina_node::core::snark::Snark, ExternalSnarkWorkerWorkError>,
) -> Option<ExternalSnarkWorkerEvent> {
    Some(match outcome {
        Outcome::Returned { .. } | Outcome::Rejected { .. } => return None,
        Outcome::WorkerStarted => ExternalSnarkWorkerEvent::Started,
        Outcome::WorkerKilled => ExternalSnarkWorkerEvent::Killed,
        Outcome::WorkerFailed { error } => {
            mina_core::error!(
                message = "isolated SNARK worker stopped",
                error = error.clone()
            );
            ExternalSnarkWorkerEvent::Error(ExternalSnarkWorkerError::Error(error))
        }
        Outcome::WorkCancelled { .. } => ExternalSnarkWorkerEvent::WorkCancelled,
        Outcome::WorkRejected { request_id, error } => {
            mina_core::warn!(
                message = "isolated SNARK work failed",
                request_id = request_id,
                error = error.clone()
            );
            ExternalSnarkWorkerEvent::WorkError(ExternalSnarkWorkerWorkError::Error(error))
        }
        Outcome::WorkReturned {
            request_id,
            kinds,
            statements,
            proofs,
            prove_ms,
            round_trip_ms,
            queued_ms,
            checker,
        } => {
            if !proofs_match_statements(&proofs, &statements) {
                let error = format!(
                    "the isolated prover's {kinds} proofs are of other statements than asked"
                );
                mina_core::error!(
                    message = "isolated SNARK work refused",
                    request_id = request_id,
                    error = error.clone()
                );
                return Some(ExternalSnarkWorkerEvent::WorkError(
                    ExternalSnarkWorkerWorkError::Error(error),
                ));
            }
            let work = mina_node::core::snark::Snark {
                snarker: checker.prover.clone(),
                fee: checker.fee.clone(),
                proofs: proofs.into(),
            };
            let times = crate::snark_worker::WorkTimes {
                kinds,
                prove_ms: round_trip_ms,
                per_job_ms: prove_ms,
                queued_ms,
            };
            match check(work, times, &checker) {
                Ok(result) => ExternalSnarkWorkerEvent::WorkResult(result),
                Err(error) => ExternalSnarkWorkerEvent::WorkError(error),
            }
        }
    })
}

fn bridge_loop(
    mut rx: mpsc::UnboundedReceiver<BridgeInput>,
    event_sender: EventSender,
    checker: Option<BlockProofChecker>,
    pending: Arc<AtomicUsize>,
    mut transport: impl FnMut(Command),
) {
    let role = if checker.is_some() {
        ProverRole::BlockProducer
    } else {
        ProverRole::SnarkWorker
    };
    let mut core = BridgeCore::new(role);
    let mut step = core.start();
    loop {
        for message in step.ignored {
            mina_core::warn!(
                message = "isolated block prover message ignored",
                detail = message
            );
        }
        for command in step.commands {
            transport(command);
        }
        for outcome in step.outcomes {
            match outcome.split() {
                Ok(outcome) => {
                    let (block_hash, result) = outcome_result(outcome, checker.as_ref());
                    pending.fetch_sub(1, Ordering::Relaxed);
                    let _ = event_sender
                        .send(BlockProducerEvent::BlockProve(block_hash, result).into());
                }
                Err(outcome) => {
                    if let Some(event) = work_event(outcome) {
                        let _ = event_sender.send(event.into());
                    }
                }
            }
        }
        let Some(input) = rx.blocking_recv() else {
            return;
        };
        step = handle_input(&mut core, input, now_ms());
    }
}

#[cfg(test)]
mod tests;
