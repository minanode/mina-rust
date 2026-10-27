#[cfg(not(target_family = "wasm"))]
mod vrf_cache;
mod vrf_evaluator;

use std::sync::Arc;

use ledger::proofs::{
    block::BlockParams, generate_block_proof, provers::BlockProver,
    transaction::debug::KimchiProofError, verifiers::BlockVerifier,
};
use mina_node::{
    account::AccountSecretKey,
    block_producer::{vrf_evaluator::VrfEvaluatorInput, BlockProducerEvent},
    core::{channels::mpsc, constants::constraint_constants, thread},
};
use mina_p2p_messages::{
    bigint::BigInt,
    binprot::{self, BinProtWrite},
    v2::{self, MinaBaseProofStableV2, ProverExtendBlockchainInputStableV2, StateHash},
};

use crate::EventSender;

pub mod isolated;
pub use isolated::IsolatedBlockProver;

type ProveSender = mpsc::TrackedUnboundedSender<ProveRequest>;

/// Where block proofs are made.
pub enum BlockProverBackend {
    /// In this process's memory, on the `mina_block_prover` thread: the
    /// native node.
    Local {
        provers: Option<BlockProver>,
        prove_sender: ProveSender,
    },
    /// In a prover with its own memory, through a byte bridge: the web node
    /// (I34c). The node's memory never holds the block prover indexes.
    Isolated(IsolatedBlockProver),
}

pub struct BlockProducerService {
    keypair: AccountSecretKey,
    vrf_evaluation_sender: mpsc::TrackedUnboundedSender<VrfEvaluatorInput>,
    prover: BlockProverBackend,
    check_proofs: bool,
}

/// A block for the native prover thread to prove and check.
pub struct ProveRequest {
    provers: BlockProver,
    block_hash: StateHash,
    input: Box<ProverExtendBlockchainInputStableV2>,
    check: bool,
}

impl BlockProducerService {
    fn new(
        keypair: AccountSecretKey,
        vrf_evaluation_sender: mpsc::TrackedUnboundedSender<VrfEvaluatorInput>,
        prove_sender: ProveSender,
        provers: Option<BlockProver>,
    ) -> Self {
        Self {
            keypair,
            vrf_evaluation_sender,
            check_proofs: true,
            prover: BlockProverBackend::Local {
                provers,
                prove_sender,
            },
        }
    }

    fn start_vrf_evaluator(
        event_sender: EventSender,
        keypair: &AccountSecretKey,
    ) -> mpsc::TrackedUnboundedSender<VrfEvaluatorInput> {
        let (vrf_evaluation_sender, vrf_evaluation_receiver) = mpsc::unbounded_channel();
        let producer_keypair = keypair.clone();
        thread::Builder::new()
            .name("mina_vrf_evaluator".to_owned())
            .spawn(move || {
                vrf_evaluator::vrf_evaluator(
                    event_sender,
                    vrf_evaluation_receiver,
                    producer_keypair.into(),
                );
            })
            .unwrap();
        vrf_evaluation_sender
    }

    pub fn skip_proof_verification(&mut self, skip: bool) {
        self.check_proofs = !skip;
    }

    pub fn start(
        event_sender: EventSender,
        keypair: AccountSecretKey,
        provers: Option<BlockProver>,
    ) -> Self {
        let vrf_evaluation_sender = Self::start_vrf_evaluator(event_sender.clone(), &keypair);
        let (prove_sender, prove_receiver) = mpsc::unbounded_channel();

        let producer_keypair = keypair.clone();
        thread::Builder::new()
            .name("mina_block_prover".to_owned())
            .spawn(move || prover_loop(producer_keypair, event_sender, prove_receiver))
            .unwrap();

        BlockProducerService::new(keypair, vrf_evaluation_sender, prove_sender, provers)
    }

    /// A producer whose block proofs are made by an isolated prover. The
    /// bridge was started, and its first prover instance with it.
    pub fn start_isolated(
        event_sender: EventSender,
        keypair: AccountSecretKey,
        prover: IsolatedBlockProver,
    ) -> Self {
        Self {
            vrf_evaluation_sender: Self::start_vrf_evaluator(event_sender, &keypair),
            keypair,
            prover: BlockProverBackend::Isolated(prover),
            check_proofs: true,
        }
    }

    pub fn keypair(&self) -> AccountSecretKey {
        self.keypair.clone()
    }

    pub fn vrf_pending_requests(&self) -> usize {
        self.vrf_evaluation_sender.len()
    }

    pub fn prove_pending_requests(&self) -> usize {
        match &self.prover {
            BlockProverBackend::Local { prove_sender, .. } => prove_sender.len(),
            BlockProverBackend::Isolated(prover) => prover.pending_requests(),
        }
    }

    /// The in-memory provers, made on first use. `None` for an isolated
    /// prover, which must never load them into this memory.
    pub fn local_provers(&self) -> Option<BlockProver> {
        match &self.prover {
            BlockProverBackend::Local { provers, .. } => {
                Some(provers.clone().unwrap_or_else(BlockProver::get_once_made))
            }
            BlockProverBackend::Isolated(_) => None,
        }
    }
}

fn prover_loop(
    keypair: AccountSecretKey,
    event_sender: EventSender,
    mut rx: mpsc::TrackedUnboundedReceiver<ProveRequest>,
) {
    // Retries now arrive as separate state-machine requests. Keep P4.6a's
    // distinct failure files instead of overwriting attempt 1 on the retry.
    let mut attempts = ProofAttemptCounter::default();
    while let Some(msg) = rx.blocking_recv() {
        let ProveRequest {
            provers,
            block_hash,
            mut input,
            check,
        } = msg.0;
        let attempt = attempts.next(&block_hash);
        set_producer_private_key(&mut input, &keypair);
        let input = &*input;
        let verifier = check.then(block_verifier).flatten();
        if check && verifier.is_none() {
            mina_core::warn!(
                message =
                    "Block verifier not loaded: our block proof is not checked before broadcasting"
            );
        }

        let res = prove_verified(
            // P15 and the genesis state machine own the retry budget.
            1,
            || prove_input(provers.clone(), input, false),
            |proof| {
                verifier
                    .as_ref()
                    .is_none_or(|verifier| verify_block_proof(&input.next_state, proof, verifier))
            },
            |_, error| {
                mina_core::error!(
                    message = "Block proof failed",
                    attempt = attempt,
                    error = format!("{error:?}")
                );
                let input = Box::new(input.clone());
                if let Err(error) =
                    dump_failed_block_proof_input(block_hash.clone(), attempt, input, error)
                {
                    mina_core::error!(
                        message = "Failure when dumping failed block proof inputs",
                        error = format!("{error}")
                    );
                }
            },
        );
        if res.is_ok() && verifier.is_some() {
            mina_core::info!(
                message = "Block proof verified in prover service",
                block_hash = block_hash.to_string()
            );
        }
        let res = res.map_err(|err| format!("{err:#}"));
        let _ = event_sender.send(BlockProducerEvent::BlockProve(block_hash, res).into());
    }
}

#[derive(Default)]
struct ProofAttemptCounter(Option<(StateHash, usize)>);

impl ProofAttemptCounter {
    fn next(&mut self, hash: &StateHash) -> usize {
        let attempt = self
            .0
            .as_ref()
            .filter(|(previous, _)| previous == hash)
            .map_or(1, |(_, attempt)| attempt.saturating_add(1));
        self.0 = Some((hash.clone(), attempt));
        attempt
    }
}

/// How many times the block producer proves a block before it gives up: once,
/// and once more if the prover fails or the proof does not verify.
///
/// The check, not the second attempt, is what keeps a proof our peers would
/// reject off the network. A second attempt can only help with a failure that
/// does not follow from the input. The one kimchi failure P4.6a could make,
/// "the permutation was not constructed correctly" for a block proved on the
/// wrong previous proof, is a broken copy constraint and fails every time.
/// Upstream's reports of that error (o1-labs/mina-rust #1350 and #977) say
/// that proving the dumped input again did not reproduce it, which leaves
/// room for a failure that does not follow from the input; nothing so far
/// tells which. The second attempt costs one more proof, and only after a
/// failure.
pub const BLOCK_PROOF_ATTEMPTS: usize = 2;

/// Proves with `prove` until `verify` accepts the proof, at most `attempts`
/// times, and reports each failed attempt (numbered from 1), a prover error or
/// a proof that does not verify, to `on_failure`. The error is the last
/// attempt's.
///
/// OCaml's block producer does not check its own proof
/// (`skip_proof_validation`), and drops the block on a prover error
/// (`handle_block_production_errors`). Checking it first means that a proof
/// our peers would reject is never broadcast.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L860-L911>
pub fn prove_verified<T>(
    attempts: usize,
    mut prove: impl FnMut() -> anyhow::Result<T>,
    mut verify: impl FnMut(&T) -> bool,
    mut on_failure: impl FnMut(usize, &anyhow::Error),
) -> anyhow::Result<T> {
    let mut last = anyhow::anyhow!("no attempt to prove the block");
    for attempt in 1..=attempts {
        let error = match prove() {
            Ok(proof) if verify(&proof) => return Ok(proof),
            Ok(_) => anyhow::anyhow!(
                "the block proof does not verify with the blockchain verification key"
            ),
            Err(error) => error,
        };
        on_failure(attempt, &error);
        last = error;
    }
    let times = if attempts == 1 { "time" } else { "times" };
    Err(last.context(format!("block proof failed {attempts} {times}")))
}

/// Whether the blockchain verifier accepts `proof` for `state`, as a peer that
/// receives the block checks it: the check `prover_loop` makes of each proof
/// before it sends it.
pub fn verify_block_proof(
    state: &v2::MinaStateProtocolStateValueStableV2,
    proof: &MinaBaseProofStableV2,
    verifier: &BlockVerifier,
) -> bool {
    ledger::proofs::verification::verify_block_proof(state, proof, verifier)
}

/// The blockchain verifier, the key peers check blocks with. Making it is
/// async in a browser, where the node made it already to verify the blocks it
/// synced, so there this takes the one made, if any.
fn block_verifier() -> Option<BlockVerifier> {
    #[cfg(not(target_family = "wasm"))]
    return Some(BlockVerifier::make());
    #[cfg(target_family = "wasm")]
    return BlockVerifier::get();
}

/// Puts the producer's private key into the input, unless it is the genesis
/// block's, which the genesis winner proves.
fn set_producer_private_key(
    input: &mut ProverExtendBlockchainInputStableV2,
    keypair: &AccountSecretKey,
) {
    if !is_genesis_input(input) {
        input.prover_state.producer_private_key = keypair.into();
    }
}

fn is_genesis_input(input: &ProverExtendBlockchainInputStableV2) -> bool {
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

pub fn prove(
    provers: BlockProver,
    input: &mut ProverExtendBlockchainInputStableV2,
    keypair: &AccountSecretKey,
    only_verify_constraints: bool,
) -> anyhow::Result<Arc<MinaBaseProofStableV2>> {
    set_producer_private_key(input, keypair);
    prove_input(provers, input, only_verify_constraints)
}

fn prove_input(
    provers: BlockProver,
    input: &ProverExtendBlockchainInputStableV2,
    only_verify_constraints: bool,
) -> anyhow::Result<Arc<MinaBaseProofStableV2>> {
    let res = generate_block_proof(BlockParams {
        input,
        block_step_prover: &provers.block_step_prover,
        block_wrap_prover: &provers.block_wrap_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        only_verify_constraints,
        expected_step_proof: None,
        ocaml_wrap_witness: None,
    });
    res.map(|proof| MinaBaseProofStableV2((&proof).into()))
        .map(Into::into)
}

impl crate::NodeService {
    /// The native block provers. Panics on a node without a producer or with
    /// an isolated prover.
    pub fn provers(&self) -> BlockProver {
        self.block_producer
            .as_ref()
            .expect("provers shouldn't be needed if block producer isn't initialized")
            .local_provers()
            .expect("an isolated block prover keeps no provers in this memory")
    }
}

impl mina_node::service::BlockProducerService for crate::NodeService {
    fn prove(
        &mut self,
        block_hash: StateHash,
        input: Box<ProverExtendBlockchainInputStableV2>,
        deadline: redux::Timestamp,
    ) {
        if self.replayer.is_some() {
            return;
        }
        let block_producer = self
            .block_producer
            .as_ref()
            .expect("prove shouldn't be requested if block producer isn't initialized");
        match &block_producer.prover {
            BlockProverBackend::Local {
                provers,
                prove_sender,
            } => {
                let provers = provers.clone().unwrap_or_else(BlockProver::get_once_made);
                let _ = prove_sender.tracked_send(ProveRequest {
                    provers,
                    block_hash,
                    input,
                    check: block_producer.check_proofs,
                });
            }
            BlockProverBackend::Isolated(prover) => {
                prover.prove(block_hash, input, u64::from(deadline) / 1_000_000)
            }
        }
    }

    fn with_producer_keypair<T>(&self, f: impl FnOnce(&AccountSecretKey) -> T) -> Option<T> {
        Some(f(&self.block_producer.as_ref()?.keypair))
    }

    fn save_block_production(&mut self) {
        if self.replayer.is_some() {
            return;
        }
        let (Some(file), Some(stats)) = (&mut self.block_production_file, &mut self.stats) else {
            return;
        };
        if let Err(error) = file.save(stats.block_producer()) {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "BlockProductionHistoryNotSaved",
                summary = "the block production history was not saved",
                path = file.path().display().to_string(),
                error = error.to_string(),
            );
        }
    }
}

/// Dumps a failed attempt's input, with the producer's key redacted, and the
/// error, to `failed_block_proof_input_<block hash>_attempt_<attempt>.binprot`
/// in the debug directory: one file per attempt.
fn dump_failed_block_proof_input(
    block_hash: StateHash,
    attempt: usize,
    mut input: Box<ProverExtendBlockchainInputStableV2>,
    error: &anyhow::Error,
) -> std::io::Result<()> {
    #[derive(binprot::macros::BinProtWrite)]
    struct DumpBlockProof {
        input: Box<ProverExtendBlockchainInputStableV2>,
        key: Vec<u8>,
        error: Vec<u8>,
        kimchi_error_with_context: Option<KimchiProofError>,
    }

    // Preserve the old dump layout, but never export the owner's key (even
    // encrypted to an upstream RSA key) or raw witnesses containing its bits.
    // Replay must supply the key privately; old encrypted-dump readers need to
    // recognise an empty key. Witness hashes remain in the error's Debug text.
    // IMPORTANT: Make sure that `input` doesn't leak the private key.
    input.prover_state.producer_private_key = v2::SignatureLibPrivateKeyStableV1(BigInt::one());

    let error_str = error.to_string();

    let input = DumpBlockProof {
        input,
        key: Vec::new(),
        error: error_str.as_bytes().to_vec(),
        kimchi_error_with_context: None,
    };

    let debug_dir = mina_core::get_debug_dir();
    let filename = debug_dir
        .join(failed_block_proof_input_filename(&block_hash, attempt))
        .to_string_lossy()
        .to_string();
    mina_core::warn!(message = "Dumping failed block proof.", filename = filename);
    std::fs::create_dir_all(&debug_dir)?;
    let mut file = std::fs::File::create(&filename)?;
    input.binprot_write(&mut file)?;
    file.sync_all()?;
    Ok(())
}

fn failed_block_proof_input_filename(block_hash: &StateHash, attempt: usize) -> String {
    format!("failed_block_proof_input_{block_hash}_attempt_{attempt}.binprot")
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// A prover whose attempts give `outcomes` in order: `Ok(true)` a proof
    /// that verifies, `Ok(false)` one that does not, `Err(())` a kimchi error.
    /// It counts its attempts and the failures reported.
    struct FakeProver {
        outcomes: RefCell<std::vec::IntoIter<Result<bool, ()>>>,
        attempts: Cell<usize>,
        failures: RefCell<Vec<(usize, String)>>,
    }

    impl FakeProver {
        fn new(outcomes: Vec<Result<bool, ()>>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into_iter()),
                attempts: Cell::new(0),
                failures: RefCell::new(Vec::new()),
            }
        }

        /// Returns the attempt whose proof was taken.
        fn run(&self) -> anyhow::Result<usize> {
            prove_verified(
                BLOCK_PROOF_ATTEMPTS,
                || {
                    let attempt = self.attempts.get() + 1;
                    self.attempts.set(attempt);
                    match self.outcomes.borrow_mut().next() {
                        Some(Ok(verifies)) => Ok((attempt, verifies)),
                        Some(Err(())) => Err(anyhow::anyhow!(
                            "kimchi error: the permutation was not constructed correctly"
                        )),
                        None => panic!("proved more often than expected"),
                    }
                },
                |&(_, verifies)| verifies,
                |attempt, error| {
                    self.failures
                        .borrow_mut()
                        .push((attempt, error.to_string()))
                },
            )
            .map(|(attempt, _)| attempt)
        }
    }

    #[test]
    fn a_proof_that_verifies_is_made_once() {
        let prover = FakeProver::new(vec![Ok(true)]);
        assert_eq!(prover.run().unwrap(), 1);
        assert_eq!(prover.attempts.get(), 1);
        assert!(prover.failures.borrow().is_empty());
    }

    #[test]
    fn a_proof_that_does_not_verify_is_made_again() {
        let prover = FakeProver::new(vec![Ok(false), Ok(true)]);
        assert_eq!(prover.run().unwrap(), 2);
        let failures = prover.failures.borrow();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, 1);
        assert!(failures[0].1.contains("does not verify"), "{failures:?}");
    }

    #[test]
    fn a_prover_error_is_proved_again() {
        let prover = FakeProver::new(vec![Err(()), Ok(true)]);
        assert_eq!(prover.run().unwrap(), 2);
        let failures = prover.failures.borrow();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].1.contains("permutation"), "{failures:?}");
    }

    /// With proof verification off, one failed attempt gives up: the node
    /// then uses a dummy proof.
    #[test]
    fn one_attempt_without_the_check_gives_up() {
        let failures = RefCell::new(Vec::new());
        let error = prove_verified(
            1,
            || Err::<(), _>(anyhow::anyhow!("kimchi error")),
            |_| true,
            |attempt, _| failures.borrow_mut().push(attempt),
        )
        .unwrap_err();
        assert_eq!(*failures.borrow(), [1]);
        assert!(format!("{error:#}").contains("block proof failed 1 time:"));
    }

    /// Each failed attempt's input is dumped to its own file.
    #[test]
    fn each_failed_attempt_is_dumped_to_its_own_file() {
        let hash: StateHash = "3NKLvHFnXhZw3mzUDz8ZdaicqVGMHEuoUYzB4YxjzKrkXLg2nYNr"
            .parse()
            .unwrap();
        let mut attempts = ProofAttemptCounter::default();
        let (first, second) = (
            failed_block_proof_input_filename(&hash, attempts.next(&hash)),
            failed_block_proof_input_filename(&hash, attempts.next(&hash)),
        );
        assert_eq!(attempts.next(&StateHash::zero()), 1);
        assert_eq!(
            first,
            "failed_block_proof_input_3NKLvHFnXhZw3mzUDz8ZdaicqVGMHEuoUYzB4YxjzKrkXLg2nYNr_attempt_1.binprot"
        );
        assert_ne!(first, second);
    }

    /// The block prover's input for block 548149 of P4.6a's offline Mesa
    /// chain (`mina-tree`'s fixture): its previous state is block 548148,
    /// with the proof of it the Rust prover made, on its genesis proof.
    const OFFLINE_548149: &[u8] =
        include_bytes!("../../../../../ledger/src/proofs/data/mesa_offline_chain_548149_input.bin");

    /// The check the node makes of its own block proof before broadcasting
    /// the block, with the live Mesa mainnet blockchain key, on a Rust block
    /// proof: it accepts the proof of block 548148 for its state, and rejects
    /// it for that state with one bit of the VRF output flipped and for the
    /// next block's state. Needs no circuit blobs: the key is embedded.
    #[test]
    fn our_block_proof_is_checked_with_the_live_key() {
        use mina_p2p_messages::binprot::BinProtRead;

        // `verify_block` dumps what fails to verify to the work directory.
        mina_core::init_work_dir_if_unset(|| {
            std::env::temp_dir().join("mina-node-common-block-producer-tests")
        });
        let _ = mina_core::NetworkConfig::init("mainnet");
        assert_eq!(
            mina_core::NetworkConfig::global().name,
            "mainnet",
            "another test set the network config first"
        );
        let input =
            ProverExtendBlockchainInputStableV2::binprot_read(&mut &*OFFLINE_548149).unwrap();
        let (state, proof) = (&input.chain.state, &input.chain.proof);
        let verifier = BlockVerifier::make();

        assert!(verify_block_proof(state, proof, &verifier));

        let mut tampered = state.clone();
        tampered.body.consensus_state.last_vrf_output.0 .0[0] ^= 1;
        assert!(!verify_block_proof(&tampered, proof, &verifier));
        assert!(!verify_block_proof(&input.next_state, proof, &verifier));
    }

    /// Two failed attempts give up, without a third: the block producer then
    /// drops the block (`BlockProducerAction::BlockProveError`), and the error
    /// is the second attempt's.
    #[test]
    fn two_failed_attempts_give_up() {
        for outcomes in [
            vec![Ok(false), Ok(false)],
            vec![Err(()), Ok(false)],
            vec![Ok(false), Err(())],
            vec![Err(()), Err(())],
        ] {
            let second_is_error = outcomes[1].is_err();
            let prover = FakeProver::new(outcomes);
            let error = format!("{:#}", prover.run().unwrap_err());
            assert_eq!(prover.attempts.get(), 2);
            let attempts: Vec<usize> = prover.failures.borrow().iter().map(|f| f.0).collect();
            assert_eq!(attempts, [1, 2]);
            assert!(error.contains("block proof failed 2 times"), "{error}");
            let second = if second_is_error {
                "permutation"
            } else {
                "does not verify"
            };
            assert!(error.contains(second), "{error}");
        }
    }
}
