//! The bridge's limits, deadlines, generations and failure handling, with
//! the public Mesa genesis transition as the block prover input.

use ledger::proofs::verifiers::BlockVerifier;
use mina_node::core::NetworkConfig;
use mina_p2p_messages::{bigint::BigInt, binprot::BinProtRead, v2};

use super::*;

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    super::encode(value).unwrap()
}

const GENESIS_INPUT: &[u8] =
    include_bytes!("../../../../../../ledger/src/proofs/data/mesa_mainnet_genesis_block_input.bin");
/// A public Mesa mainnet block header, as gossiped, with OCaml's proof.
const MAINNET_563978: &[u8] =
    include_bytes!("../../../../../../web-verifier/tests/fixtures/mainnet-563978.gossipbin");

const NOW: u64 = 1_800_000_000_000;
const DEADLINE: u64 = NOW + 90_000;

fn mainnet() {
    let _ = NetworkConfig::init("mainnet");
    assert_eq!(
        NetworkConfig::global().name,
        "mainnet",
        "another test set the network first"
    );
}

fn input() -> v2::ProverExtendBlockchainInputStableV2 {
    decode_exact(GENESIS_INPUT, MAX_REQUEST_BYTES).unwrap()
}

fn hash(n: u8) -> v2::StateHash {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    v2::DataHashLibStateHashStableV1(BigInt::from_bytes(bytes)).into()
}

fn proof() -> v2::MinaBaseProofStableV2 {
    (*ledger::dummy::dummy_blockchain_proof()).clone()
}

fn response(request_id: u64, block_hash: v2::StateHash) -> Vec<u8> {
    encode(&ProveResponse {
        version: BRIDGE_VERSION,
        request_id,
        block_hash,
        proof: proof(),
    })
}

/// A block producer's started core whose first instance is ready.
fn ready() -> BridgeCore {
    ready_as(ProverRole::BlockProducer)
}

fn ready_as(role: ProverRole) -> BridgeCore {
    let mut core = BridgeCore::new(role);
    assert_eq!(
        core.start().commands,
        vec![Command::Start { generation: 1 }]
    );
    assert!(core.host_ready(1, NOW).commands.is_empty());
    assert_eq!(core.host(), &HostState::Ready(1));
    core
}

fn submit(core: &mut BridgeCore, n: u8, deadline: u64) -> Step {
    core.submit(hash(n), input(), false, deadline, NOW)
}

fn prove_command(step: &Step) -> (u64, u64, &[u8]) {
    match step.commands.as_slice() {
        [Command::Prove {
            generation,
            request_id,
            bytes,
            ..
        }] => (*generation, *request_id, bytes),
        other => panic!("expected one prove command, got {other:?}"),
    }
}

fn rejected(step: &Step) -> Vec<(v2::StateHash, String)> {
    step.outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::Rejected {
                block_hash, error, ..
            } => Some((block_hash.clone(), error.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_request_holds_the_input_without_the_producer_key() {
    let mut core = ready();
    let mut with_key = input();
    with_key.prover_state.producer_private_key =
        v2::SignatureLibPrivateKeyStableV1(BigInt::from_bytes([7u8; 32]));
    let step = core.submit(hash(1), with_key, false, DEADLINE, NOW);
    let (generation, request_id, bytes) = prove_command(&step);
    assert_eq!((generation, request_id), (1, 1));
    assert!(bytes.len() <= MAX_REQUEST_BYTES);
    let request: ProveRequest = decode_exact(bytes, MAX_REQUEST_BYTES).unwrap();
    assert_eq!(request.version, BRIDGE_VERSION);
    assert_eq!(request.block_hash, hash(1));
    assert_eq!(request.deadline_ms, DEADLINE);
    assert!(!request.is_genesis);
    assert_eq!(
        request.input.prover_state.producer_private_key,
        v2::SignatureLibPrivateKeyStableV1(BigInt::one()),
        "the key must never leave the node in a request"
    );
    assert_eq!(request.input.next_state, input().next_state);
    assert_eq!(core.pending(), 1);
}

#[test]
fn the_mesa_genesis_transition_is_recognised_and_bounded_by_its_slot() {
    mainnet();
    let input = input();
    assert!(is_genesis_input(&input));
    assert_eq!(
        slot_deadline_ms(&input),
        input.next_state.body.blockchain_state.timestamp.as_u64() + 90_000
    );
    let mut core = ready();
    let step = handle_input(
        &mut core,
        BridgeInput::Submit {
            block_hash: hash(1),
            input: Box::new(input),
            deadline_ms: DEADLINE,
        },
        NOW,
    );
    let (_, _, bytes) = prove_command(&step);
    let request: ProveRequest = decode_exact(bytes, MAX_REQUEST_BYTES).unwrap();
    assert!(request.is_genesis);
    assert_eq!(request.deadline_ms, DEADLINE);
}

#[test]
fn an_oversized_request_is_rejected_before_it_is_sent() {
    let mut core = ready();
    let mut big = input();
    let index = big.prover_state.ledger.indexes.front().cloned().unwrap();
    // Each entry is an account ID and an index: about 70 bytes.
    big.prover_state.ledger.indexes = std::iter::repeat_n(index, 20_000).collect();
    let step = core.submit(hash(1), big, false, DEADLINE, NOW);
    assert!(step.commands.is_empty());
    let rejected = rejected(&step);
    assert_eq!(rejected.len(), 1);
    assert!(rejected[0].1.contains("exceeds"), "{}", rejected[0].1);
    assert_eq!(core.pending(), 0);
}

#[test]
fn decoding_is_exact_and_bounded() {
    assert!(decode_exact::<ProveResponse>(&[], MAX_RESPONSE_BYTES).is_err());
    let mut trailing = response(1, hash(1));
    trailing.push(0);
    let error = decode_exact::<ProveResponse>(&trailing, MAX_RESPONSE_BYTES).unwrap_err();
    assert!(error.contains("trailing"), "{error}");
    let error = decode_exact::<ProveResponse>(&vec![0; MAX_RESPONSE_BYTES + 1], MAX_RESPONSE_BYTES)
        .unwrap_err();
    assert!(error.contains("exceeds"), "{error}");
}

#[test]
fn a_matching_response_returns_the_expected_state_for_verification() {
    let mut core = ready();
    let step = submit(&mut core, 1, DEADLINE);
    let (generation, request_id, _) = prove_command(&step);
    let step = core.response(
        generation,
        request_id,
        &response(request_id, hash(1)),
        NOW + 40_000,
    );
    match step.outcomes.as_slice() {
        [Outcome::Returned {
            block_hash,
            expected_state,
            proof: returned,
            prove_ms,
            ..
        }] => {
            assert_eq!(block_hash, &hash(1));
            assert_eq!(**expected_state, input().next_state);
            assert_eq!(**returned, proof());
            assert_eq!(*prove_ms, 40_000);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(core.pending(), 0);
    assert_eq!(core.host(), &HostState::Ready(1));
}

#[test]
fn stale_responses_are_ignored() {
    let mut core = ready();
    let step = submit(&mut core, 1, DEADLINE);
    let (generation, request_id, _) = prove_command(&step);
    // Another instance, another request ID.
    let step = core.response(
        generation + 1,
        request_id,
        &response(request_id, hash(1)),
        NOW,
    );
    assert!(step.outcomes.is_empty() && step.ignored.len() == 1);
    let step = core.response(
        generation,
        request_id + 1,
        &response(request_id + 1, hash(1)),
        NOW,
    );
    assert!(step.outcomes.is_empty() && step.ignored.len() == 1);
    let step = core.prove_error(generation + 1, request_id, "late", NOW);
    assert!(step.outcomes.is_empty() && step.ignored.len() == 1);
    assert_eq!(core.pending(), 1, "the job still waits for its own answer");
}

#[test]
fn a_late_response_cannot_complete_a_newer_request() {
    let mut core = ready();
    let step = submit(&mut core, 1, DEADLINE);
    let (generation, first, _) = prove_command(&step);
    // The instance dies; the replacement gets the next block.
    let step = core.host_failed(generation, "worker error");
    assert_eq!(rejected(&step).len(), 1);
    assert!(core.host_ready(2, NOW).commands.is_empty());
    let step = submit(&mut core, 2, DEADLINE);
    let (generation_2, second, _) = prove_command(&step);
    assert_eq!(generation_2, 2);
    // The dead instance's answer for the first block arrives.
    let step = core.response(generation, first, &response(first, hash(1)), NOW);
    assert!(step.outcomes.is_empty());
    let step = core.response(generation_2, second, &response(second, hash(2)), NOW);
    assert!(matches!(
        step.outcomes.as_slice(),
        [Outcome::Returned { block_hash, .. }] if *block_hash == hash(2)
    ));
}

#[test]
fn a_response_for_another_block_or_too_large_retires_the_instance() {
    for bad in [
        response(1, hash(9)),
        vec![0; MAX_RESPONSE_BYTES + 1],
        {
            let mut bytes = response(1, hash(1));
            bytes.push(0);
            bytes
        },
        encode(&ProveResponse {
            version: BRIDGE_VERSION + 1,
            request_id: 1,
            block_hash: hash(1),
            proof: proof(),
        }),
    ] {
        let mut core = ready();
        let step = submit(&mut core, 1, DEADLINE);
        let (generation, request_id, _) = prove_command(&step);
        let step = core.response(generation, request_id, &bad, NOW);
        let rejected = rejected(&step);
        assert_eq!(rejected.len(), 1);
        assert!(
            rejected[0].1.contains("invalid block prover response"),
            "{}",
            rejected[0].1
        );
        assert_eq!(
            step.commands,
            vec![
                Command::Retire {
                    generation: 1,
                    reason: "invalid response".to_owned()
                },
                Command::Start { generation: 2 }
            ]
        );
    }
}

#[test]
fn instance_death_rejects_active_and_queued_jobs_at_once() {
    let mut core = ready();
    let _ = submit(&mut core, 1, DEADLINE);
    let step = submit(&mut core, 2, DEADLINE);
    assert!(step.commands.is_empty(), "one job at a time");
    assert_eq!(core.pending(), 2);
    let step = core.host_failed(1, "prover iframe removed");
    let rejected = rejected(&step);
    assert_eq!(
        rejected.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
        vec![hash(1), hash(2)]
    );
    assert!(rejected
        .iter()
        .all(|r| r.1.contains("prover iframe removed")));
    assert_eq!(
        step.commands,
        vec![
            Command::Retire {
                generation: 1,
                reason: "prover iframe removed".to_owned()
            },
            Command::Start { generation: 2 }
        ]
    );
    assert_eq!(core.pending(), 0);
    // A second report of the same death changes nothing.
    let step = core.host_failed(1, "prover host unloaded");
    assert!(step.outcomes.is_empty() && step.commands.is_empty());
}

#[test]
fn a_request_waits_for_a_starting_instance_until_its_deadline() {
    let mut core = BridgeCore::new(ProverRole::BlockProducer);
    let _ = core.start();
    let step = submit(&mut core, 1, DEADLINE);
    assert!(step.commands.is_empty() && step.outcomes.is_empty());
    assert!(core.tick(DEADLINE - 1).outcomes.is_empty());
    let step = core.tick(DEADLINE);
    let rejected = rejected(&step);
    assert_eq!(rejected.len(), 1);
    assert!(rejected[0].1.contains("deadline"));
    assert!(step.commands.is_empty(), "the starting instance is kept");
    // Or, ready in time, it is sent then.
    let _ = submit(&mut core, 2, DEADLINE + 90_000);
    let step = core.host_ready(1, NOW);
    assert_eq!(prove_command(&step).0, 1);
}

#[test]
fn a_proof_that_misses_its_deadline_retires_the_instance() {
    let mut core = ready();
    let _ = submit(&mut core, 1, DEADLINE);
    assert!(core.tick(DEADLINE - 1).outcomes.is_empty());
    let step = core.tick(DEADLINE);
    assert_eq!(rejected(&step).len(), 1);
    assert_eq!(
        step.commands,
        vec![
            Command::Retire {
                generation: 1,
                reason: "block proof missed its deadline".to_owned()
            },
            Command::Start { generation: 2 }
        ]
    );
}

#[test]
fn a_request_past_its_deadline_is_rejected_at_once() {
    let mut core = ready();
    let step = submit(&mut core, 1, NOW);
    assert!(step.commands.is_empty());
    assert_eq!(rejected(&step).len(), 1);
}

#[test]
fn a_newer_request_supersedes_the_queued_one() {
    let mut core = ready();
    let _ = submit(&mut core, 1, DEADLINE);
    let _ = submit(&mut core, 2, DEADLINE);
    let step = submit(&mut core, 3, DEADLINE);
    let rejected = rejected(&step);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].0, hash(2));
    assert!(rejected[0].1.contains("superseded"));
    assert_eq!(core.pending(), 2);
}

#[test]
fn a_prover_error_keeps_the_instance_and_sends_the_next_job() {
    let mut core = ready();
    let step = submit(&mut core, 1, DEADLINE);
    let (generation, request_id, _) = prove_command(&step);
    let _ = submit(&mut core, 2, DEADLINE);
    let step = core.prove_error(generation, request_id, "constraints not satisfied", NOW);
    assert_eq!(rejected(&step).len(), 1);
    let (next_generation, next_request, _) = prove_command(&step);
    assert_eq!((next_generation, next_request), (1, 2));
    assert_eq!(core.host(), &HostState::Ready(1));
}

#[test]
fn repeated_instance_failures_disable_proving() {
    let mut core = BridgeCore::new(ProverRole::BlockProducer);
    let _ = core.start();
    for generation in 1..MAX_CONSECUTIVE_HOST_FAILURES as u64 {
        let step = core.host_failed(generation, "setup failed");
        assert_eq!(
            step.commands.last(),
            Some(&Command::Start {
                generation: generation + 1
            })
        );
    }
    let step = core.host_failed(MAX_CONSECUTIVE_HOST_FAILURES as u64, "setup failed");
    assert!(!step
        .commands
        .iter()
        .any(|c| matches!(c, Command::Start { .. })));
    assert!(matches!(core.host(), HostState::Disabled(_)));
    let step = submit(&mut core, 1, DEADLINE);
    let rejected = rejected(&step);
    assert_eq!(rejected.len(), 1);
    assert!(rejected[0].1.contains("disabled"));
}

#[test]
fn a_good_proof_resets_the_failure_budget() {
    let mut core = ready();
    let _ = core.host_failed(1, "one");
    let _ = core.host_ready(2, NOW);
    let step = submit(&mut core, 1, DEADLINE);
    let (generation, request_id, _) = prove_command(&step);
    let _ = core.response(generation, request_id, &response(request_id, hash(1)), NOW);
    for generation in 2..2 + MAX_CONSECUTIVE_HOST_FAILURES as u64 - 1 {
        let _ = core.host_failed(generation, "again");
    }
    assert!(
        matches!(core.host(), HostState::Starting(_)),
        "{:?}",
        core.host()
    );
}

#[test]
fn a_stale_ready_is_ignored() {
    let mut core = ready();
    let _ = core.host_failed(1, "gone");
    let step = core.host_ready(1, NOW);
    assert_eq!(step.ignored.len(), 1);
    assert_eq!(core.host(), &HostState::Starting(2));
}

#[test]
fn a_request_without_a_started_prover_is_rejected() {
    let mut core = BridgeCore::new(ProverRole::BlockProducer);
    let step = submit(&mut core, 1, DEADLINE);
    assert_eq!(rejected(&step).len(), 1);
}

#[test]
fn returned_proofs_are_verified_against_the_expected_state() {
    mainnet();
    mina_node::core::init_work_dir_if_unset(|| std::env::temp_dir().join("i34c-bridge-tests"));
    assert_eq!(MAINNET_563978[8], 0, "a public NewState message");
    let header = v2::MinaBlockHeaderStableV2::binprot_read(&mut &MAINNET_563978[9..]).unwrap();
    let verifier = BlockProofChecker {
        verifier: BlockVerifier::make(),
        srs: mina_node::snark::get_srs(),
    };
    let proof = header.protocol_state_proof.clone();
    assert!(verify_returned_proof(
        &header.protocol_state,
        &proof,
        &verifier
    ));
    // Another state than the one proved.
    let mut other = header.protocol_state.clone();
    other.body.genesis_state_hash = other.previous_state_hash.clone();
    assert!(!verify_returned_proof(&other, &proof, &verifier));
    // Not a proof of this state at all: the dummy proof.
    assert!(!verify_returned_proof(
        &header.protocol_state,
        &ledger::dummy::dummy_blockchain_proof(),
        &verifier
    ));
    // And through the outcome the node receives.
    let (block_hash, result) = outcome_result(
        BlockOutcome::Returned {
            block_hash: hash(1),
            request_id: 1,
            expected_state: Box::new(other),
            proof: proof.clone(),
            prove_ms: 0,
            deadline_ms: u64::MAX,
        },
        Some(&verifier),
    );
    assert_eq!(block_hash, hash(1));
    assert!(result.unwrap_err().contains("does not verify"));
    let (_, result) = outcome_result(
        BlockOutcome::Returned {
            block_hash: hash(1),
            request_id: 1,
            expected_state: Box::new(header.protocol_state.clone()),
            proof: proof.clone(),
            prove_ms: 0,
            deadline_ms: u64::MAX,
        },
        Some(&verifier),
    );
    assert_eq!(result.unwrap(), proof);
}

#[test]
fn a_response_at_or_after_deadline_is_rejected_without_waiting_for_tick() {
    for received in [DEADLINE, DEADLINE + 1] {
        let mut core = ready();
        let _ = submit(&mut core, 1, DEADLINE);
        let step = core.response(1, 1, &response(1, hash(1)), received);
        assert_eq!(rejected(&step).len(), 1);
        assert!(rejected(&step)[0].1.contains("deadline"));
        assert_eq!(core.pending(), 0);
        assert_eq!(core.host(), &HostState::Ready(1));
        assert!(step.commands.is_empty());
    }
}

#[test]
fn dispatch_reserves_configured_proof_time_and_margin() {
    for (remaining, dispatches) in [(0, false), (1, false), (44_999, false), (45_000, true)] {
        let mut core = ready();
        let step = submit(&mut core, 1, NOW + remaining);
        assert_eq!(!step.commands.is_empty(), dispatches);
        assert_eq!(rejected(&step).len(), usize::from(!dispatches));
        assert_eq!(core.host(), &HostState::Ready(1));
        assert_eq!(core.consecutive_failures, 0);
    }
    let mut core = ready();
    handle_input(
        &mut core,
        BridgeInput::SetProofBudget(ProofBudget {
            proof_ms: 60_000,
            margin_ms: 10_000,
        }),
        NOW,
    );
    assert_eq!(rejected(&submit(&mut core, 1, NOW + 69_999)).len(), 1);
    prove_command(&submit(&mut core, 2, NOW + 70_000));
}

#[test]
fn a_slow_replacement_keeps_its_health_and_failure_budget() {
    let mut core = ready();
    submit(&mut core, 1, NOW + 95_000);
    core.host_failed(1, "prover iframe removed");
    submit(&mut core, 1, NOW + 95_000);
    let step = core.host_ready(2, NOW + 91_000);
    assert_eq!(rejected(&step).len(), 1);
    assert!(rejected(&step)[0].1.contains("4000 ms left"));
    assert!(step.commands.is_empty());
    assert_eq!(core.pending(), 0);
    assert_eq!(core.consecutive_failures, 1);
    assert_eq!(core.host(), &HostState::Ready(2));
    // A later slot uses this instance without another setup.
    let step = core.submit(hash(2), input(), false, NOW + 190_000, NOW + 91_001);
    assert_eq!(prove_command(&step).0, 2);
}

#[test]
fn verification_cannot_deliver_a_proof_after_deadline() {
    for (start, finish, expected) in [
        (DEADLINE - 2, DEADLINE - 1, "ok"),
        (DEADLINE - 1, DEADLINE, "during verification"),
        (DEADLINE - 1, DEADLINE + 1, "during verification"),
        (DEADLINE, DEADLINE, "before verification"),
    ] {
        let outcome = BlockOutcome::Returned {
            block_hash: hash(1),
            request_id: 1,
            expected_state: Box::new(input().next_state),
            proof: Arc::new(proof()),
            prove_ms: 0,
            deadline_ms: DEADLINE,
        };
        let mut times = [start, finish].into_iter();
        let mut calls = 0;
        // Timing test only: cryptographic verification is tested above using
        // an OCaml mainnet proof and the live key.
        let (_, result) = outcome_result_with(
            outcome,
            |_, _| {
                calls += 1;
                true
            },
            || times.next().unwrap(),
        );
        if expected == "ok" {
            assert!(result.is_ok());
        } else {
            assert!(result.unwrap_err().contains(expected));
        }
        assert_eq!(calls, usize::from(start < DEADLINE));
    }
}

#[test]
fn genesis_errors_on_first_and_retry_attempt_leave_no_bridge_job() {
    for death in [false, true] {
        let mut core = ready();
        for attempt in 1..=2 {
            let generation = if death { attempt } else { 1 };
            let step = core.submit(hash(1), input(), true, DEADLINE, NOW);
            assert_eq!(prove_command(&step).0, generation);
            let step = if death {
                core.host_failed(generation, "prover iframe removed")
            } else {
                core.prove_error(generation, attempt, "injected genesis proof failure", NOW)
            };
            assert_eq!(rejected(&step).len(), 1);
            assert_eq!(core.pending(), 0);
            if death {
                core.host_ready(generation + 1, NOW);
            }
        }
    }
}

// P5.3: SNARK work in the same prover instance.

/// OCaml's own Mesa mainnet works, from R24c's capture of an OCaml 4.0.x
/// seed's `get_completed_snarks` answer: each has the fee and prover its
/// proofs' sok digests commit to.
fn ocaml_works() -> Vec<v2::TransactionSnarkWorkTStableV2> {
    use mina_p2p_messages::{
        rpc::GetCompletedSnarksV1,
        rpc_kernel::{ResponsePayload, RpcMethod},
    };
    const CAPTURE: &[u8] = include_bytes!(
        "../../../../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin"
    );
    type Response = <GetCompletedSnarksV1 as RpcMethod>::Response;
    let payload = ResponsePayload::<Response>::binprot_read(&mut &*CAPTURE).unwrap();
    payload.0.unwrap().0.unwrap().0.into_iter().collect()
}

fn proof_statements(
    proofs: &v2::TransactionSnarkWorkTStableV2Proofs,
) -> Vec<v2::MinaStateSnarkedLedgerStateStableV2> {
    let proofs = match proofs {
        v2::TransactionSnarkWorkTStableV2Proofs::One(one) => vec![one],
        v2::TransactionSnarkWorkTStableV2Proofs::Two((one, two)) => vec![one, two],
    };
    proofs
        .iter()
        .map(|proof| {
            let s = &proof.0.statement;
            v2::MinaStateSnarkedLedgerStateStableV2(
                v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement {
                    source: s.source.clone(),
                    target: s.target.clone(),
                    connecting_ledger_left: s.connecting_ledger_left.clone(),
                    connecting_ledger_right: s.connecting_ledger_right.clone(),
                    supply_increase: s.supply_increase.clone(),
                    fee_excess: s.fee_excess.clone(),
                    sok_digest: (),
                },
            )
        })
        .collect()
}

/// A merge spec over the two proofs of an OCaml `Two` work: real statements
/// and proofs, in a spec of the shape the worker proves. Its statement is
/// the first proof's (the bridge never checks merge arithmetic: the prover
/// does, and the self-check verifies what comes back).
fn merge_spec() -> SnarkWorkSpec {
    let work = ocaml_works()
        .into_iter()
        .find(|work| matches!(work.proofs, v2::TransactionSnarkWorkTStableV2Proofs::Two(_)))
        .expect("the capture has two-proof works");
    let v2::TransactionSnarkWorkTStableV2Proofs::Two((left, right)) = work.proofs else {
        unreachable!()
    };
    let statement =
        proof_statements(&v2::TransactionSnarkWorkTStableV2Proofs::One(left.clone())).remove(0);
    SnarkWorkSpec::One(
        v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Merge(Box::new((
            statement, left, right,
        ))),
    )
}

/// A checker for the public genesis winner's key and 0.1 MINA, with the
/// mainnet transaction key.
fn checker() -> WorkChecker {
    mainnet();
    WorkChecker {
        fee: v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
            100_000_000.into(),
        )),
        prover: "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg"
            .parse()
            .unwrap(),
        verifier: ledger::proofs::verifiers::TransactionVerifier::make(),
        srs: mina_node::snark::get_srs(),
    }
}

/// A SNARK worker's ready core whose worker runs.
fn working() -> BridgeCore {
    let mut core = ready_as(ProverRole::SnarkWorker);
    let step = core.start_worker(checker());
    assert!(matches!(step.outcomes.as_slice(), [Outcome::WorkerStarted]));
    core
}

#[test]
fn pc2_unavailable_emitted_and_merge_proofs_reject_requests_without_dispatch() {
    use mina_p2p_messages::proof_cache::ProofStore;
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../target/test-data/proof-cache/bridge-{}",
        std::process::id()
    ));
    let store = ProofStore::dir(&dir).unwrap();
    let work = ocaml_works().remove(0);
    let proof = match &work.proofs {
        v2::TransactionSnarkWorkTStableV2Proofs::One(p) => p,
        v2::TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => p,
    };
    proof.0.proof.cache_in(&store).unwrap();
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(file).unwrap();
    let mut input = input();
    input.ledger_proof = Some(std::sync::Arc::new(proof.clone()));
    let mut core = ready();
    let step = core.submit(hash(1), input, false, DEADLINE, NOW);
    assert!(step.commands.is_empty());
    assert!(
        matches!(step.outcomes.as_slice(), [Outcome::Rejected { error, .. }] if error.contains("unavailable"))
    );
    assert_eq!(core.pending(), 0);
    drop(work);

    let spec = merge_spec();
    let SnarkWorkSpec::One(v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Merge(
        merge,
    )) = &spec
    else {
        unreachable!()
    };
    merge.1 .0.proof.cache_in(&store).unwrap();
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(file).unwrap();
    let mut core = working();
    let step = core.submit_work(spec, NOW);
    assert!(step.commands.is_empty());
    assert!(
        matches!(step.outcomes.as_slice(), [Outcome::WorkRejected { error, .. }] if error.contains("unavailable"))
    );
    assert_eq!(core.pending_work(), 0);
    assert_eq!(store.stats().files, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

fn work_command(step: &Step) -> (u64, u64, &[u8]) {
    match step.commands.as_slice() {
        [Command::ProveWork {
            generation,
            request_id,
            bytes,
            timeout_ms,
        }] => {
            assert_eq!(*timeout_ms, WORK_TIMEOUT_MS);
            (*generation, *request_id, bytes)
        }
        other => panic!("expected one work command, got {other:?}"),
    }
}

/// The prover's answer to a work request, with `proofs` (OCaml's).
fn work_response(request_id: u64, proofs: v2::TransactionSnarkWorkTStableV2Proofs) -> Vec<u8> {
    let jobs = match &proofs {
        v2::TransactionSnarkWorkTStableV2Proofs::One(_) => 1,
        v2::TransactionSnarkWorkTStableV2Proofs::Two(_) => 2,
    };
    encode(&WorkResponse {
        version: BRIDGE_VERSION,
        request_id,
        proofs,
        prove_ms: vec![7_000; jobs],
    })
}

fn one_proof() -> v2::TransactionSnarkWorkTStableV2Proofs {
    let SnarkWorkSpec::One(v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Merge(
        merge,
    )) = merge_spec()
    else {
        unreachable!()
    };
    v2::TransactionSnarkWorkTStableV2Proofs::One(merge.1.clone())
}

fn work_outcomes(step: &Step) -> Vec<&Outcome> {
    step.outcomes
        .iter()
        .filter(|o| !matches!(o, Outcome::Returned { .. } | Outcome::Rejected { .. }))
        .collect()
}

#[test]
fn a_work_request_carries_the_spec_fee_and_public_key_only() {
    let mut core = working();
    let step = core.submit_work(merge_spec(), NOW);
    let (generation, request_id, bytes) = work_command(&step);
    assert_eq!((generation, request_id), (1, 1));
    let request: WorkRequest = decode_exact(bytes, MAX_REQUEST_BYTES).unwrap();
    assert_eq!(request.version, BRIDGE_VERSION);
    assert_eq!(request.fee, checker().fee);
    assert_eq!(request.prover, checker().prover);
    assert_eq!(encode(&request.spec), encode(&merge_spec()));
    assert_eq!(core.pending_work(), 1);
    assert_eq!(core.pending(), 0);
}

/// A block job never waits behind SNARK work (P5.3's review): a block
/// producer's prover refuses the SNARK worker, so whenever a won slot
/// becomes known, early or late, and however long a SNARK job would take,
/// its block job is sent the moment it is submitted.
#[test]
fn a_block_producers_prover_refuses_the_snark_worker() {
    let mut core = ready();
    let step = handle_input(
        &mut core,
        BridgeInput::StartWorker(Box::new(checker())),
        NOW,
    );
    assert!(step.commands.is_empty());
    assert!(matches!(
        step.outcomes.as_slice(),
        [Outcome::WorkerFailed { error }] if error == WORKER_NEEDS_OWN_PROVER
    ));
    // The worker hears it as a worker error, not a job error, so it stops
    // rather than take the next job.
    let event = work_event(step.outcomes.into_iter().next().unwrap());
    assert!(matches!(
        event,
        Some(ExternalSnarkWorkerEvent::Error(ExternalSnarkWorkerError::Error(error)))
            if error == WORKER_NEEDS_OWN_PROVER
    ));

    // Work submitted anyway is refused and never reaches the instance.
    for _ in 0..3 {
        let step = handle_input(
            &mut core,
            BridgeInput::SubmitWork(Box::new(merge_spec())),
            NOW,
        );
        assert!(step.commands.is_empty());
        assert!(matches!(
            work_outcomes(&step).as_slice(),
            [Outcome::WorkRejected { .. }]
        ));
    }
    assert_eq!(core.pending_work(), 0);

    // A slot learned late, with only the proof budget left: its block job
    // is sent at once, in the step that submits it.
    let deadline = NOW + ProofBudget::default().minimum_remaining_ms();
    let step = submit(&mut core, 1, deadline);
    let (generation, request, _) = prove_command(&step);
    assert_eq!(generation, 1);
    assert!(rejected(&step).is_empty());
    let step = core.response(1, request, &response(request, hash(1)), deadline - 1);
    assert!(matches!(
        step.outcomes.as_slice(),
        [Outcome::Returned { .. }]
    ));

    // While the instance starts, a block job is the only job it gets.
    let mut core = BridgeCore::new(ProverRole::BlockProducer);
    core.start();
    assert!(matches!(
        core.start_worker(checker()).outcomes.as_slice(),
        [Outcome::WorkerFailed { .. }]
    ));
    assert!(core.submit_work(merge_spec(), NOW).commands.is_empty());
    assert!(submit(&mut core, 2, DEADLINE).commands.is_empty());
    prove_command(&core.host_ready(1, NOW + 5_000));
    assert_eq!(core.pending_work(), 0);
}

/// The other side: a SNARK worker's prover proves no block, so it never
/// has a block job to delay either.
#[test]
fn a_snark_workers_prover_refuses_block_jobs() {
    let mut core = working();
    assert_eq!(core.role(), ProverRole::SnarkWorker);
    let (_, work_request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    let step = submit(&mut core, 1, DEADLINE);
    assert!(step.commands.is_empty());
    assert_eq!(
        rejected(&step),
        vec![(hash(1), "this prover proves SNARK work only".to_owned())]
    );
    assert_eq!(core.pending(), 0);
    let step = core.response(
        1,
        work_request,
        &work_response(work_request, one_proof()),
        NOW + 30_000,
    );
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkReturned { .. }]
    ));
    assert!(step.commands.is_empty());
}

#[test]
fn work_without_a_running_worker_or_prover_is_refused() {
    let mut core = ready();
    let step = core.submit_work(merge_spec(), NOW);
    assert!(step.commands.is_empty());
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkRejected { .. }]
    ));
    let mut core = BridgeCore::new(ProverRole::SnarkWorker);
    assert!(matches!(
        core.start_worker(checker()).outcomes.as_slice(),
        [Outcome::WorkerFailed { .. }]
    ));
}

/// A queued job is dropped at once; one being proved cannot be stopped, and
/// its proof still comes back (and is offered, as P5.2's in-process worker's
/// is). A killed worker's late answer is dropped.
#[test]
fn cancel_and_kill_follow_the_in_process_worker() {
    let mut core = working();
    let (_, request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    let step = core.cancel_work();
    assert!(
        step.outcomes.is_empty(),
        "a proof being made is not cancelled"
    );
    let step = core.response(1, request, &work_response(request, one_proof()), NOW + 1);
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkReturned { .. }]
    ));

    // Queued while the instance starts: dropped.
    let mut core = BridgeCore::new(ProverRole::SnarkWorker);
    core.start();
    assert!(matches!(
        core.start_worker(checker()).outcomes.as_slice(),
        [Outcome::WorkerStarted]
    ));
    assert!(core.submit_work(merge_spec(), NOW).commands.is_empty());
    let step = core.cancel_work();
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkCancelled { .. }]
    ));
    assert_eq!(core.pending_work(), 0);

    // Killed while proving: no answer reaches the worker.
    let mut core = working();
    let (_, request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    assert!(matches!(
        core.kill_worker().outcomes.as_slice(),
        [Outcome::WorkerKilled]
    ));
    let step = core.response(1, request, &work_response(request, one_proof()), NOW + 1);
    assert!(work_outcomes(&step).is_empty());
    assert_eq!(core.host(), &HostState::Ready(1));
}

/// An instance that dies loses the work it proves, and a queued job waits
/// for the replacement; three failures in a row stop the worker for good,
/// with a worker error rather than a job error (which would only make it
/// take the next job).
#[test]
fn instance_failures_lose_the_work_being_proved_then_stop_the_worker() {
    let mut core = working();
    let (_, request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    let step = core.host_failed(1, "prover iframe removed");
    assert!(
        matches!(work_outcomes(&step).as_slice(), [Outcome::WorkRejected { request_id, .. }] if *request_id == request)
    );
    assert!(core.submit_work(merge_spec(), NOW).commands.is_empty());
    work_command(&core.host_ready(2, NOW));
    core.host_failed(2, "prover worker error");
    let step = core.host_failed(3, "prover worker error");
    assert!(matches!(core.host(), HostState::Disabled(_)));
    assert!(work_outcomes(&step)
        .iter()
        .any(|o| matches!(o, Outcome::WorkerFailed { .. })));
    assert!(matches!(
        work_outcomes(&core.submit_work(merge_spec(), NOW)).as_slice(),
        [Outcome::WorkerFailed { .. }]
    ));
}

#[test]
fn a_bad_or_late_work_response_retires_the_instance() {
    // Two proofs for one job.
    let mut core = working();
    let (_, request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    let two = match &merge_spec() {
        SnarkWorkSpec::One(
            v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Merge(merge),
        ) => v2::TransactionSnarkWorkTStableV2Proofs::Two((merge.1.clone(), merge.2.clone())),
        _ => unreachable!(),
    };
    let step = core.response(1, request, &work_response(request, two), NOW + 1);
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkRejected { .. }]
    ));
    assert!(step.commands.contains(&Command::Retire {
        generation: 1,
        reason: "invalid response".to_owned()
    }));
    assert_eq!(core.host(), &HostState::Starting(2));

    // Stale: another instance's or another request's answer is ignored.
    let mut core = working();
    let (_, request, _) = work_command(&core.submit_work(merge_spec(), NOW));
    for (generation, id) in [(2, request), (1, request + 1)] {
        let step = core.response(generation, id, &work_response(id, one_proof()), NOW + 1);
        assert!(step.outcomes.is_empty() && step.ignored.len() == 1);
    }

    // Hung: retired after the work timeout.
    assert!(core.tick(NOW + WORK_TIMEOUT_MS - 1).commands.is_empty());
    let step = core.tick(NOW + WORK_TIMEOUT_MS);
    assert!(matches!(
        work_outcomes(&step).as_slice(),
        [Outcome::WorkRejected { .. }]
    ));
    assert_eq!(core.host(), &HostState::Starting(2));
}

/// The largest OCaml bundle, two proofs, as a work response, fits the
/// response limit with room.
#[test]
fn the_largest_ocaml_bundle_fits_the_response_limit() {
    let largest = ocaml_works()
        .into_iter()
        .map(|work| work_response(1, work.proofs).len())
        .max()
        .unwrap();
    assert!(largest * 2 <= MAX_RESPONSE_BYTES, "{largest} bytes");
}

/// The self-check on the isolated path is P5.2's, with real OCaml proofs and
/// the live mainnet transaction key: a work whose proofs are of the
/// statements asked for and commit to its own fee and prover is offered;
/// the same proofs for another fee or prover are refused before any offer,
/// as are proofs of other statements.
#[test]
fn returned_work_is_offered_only_if_it_passes_the_self_check() {
    let base = checker();
    mina_node::core::init_work_dir_if_unset(|| std::env::temp_dir().join("p53-bridge-tests"));
    // The capture's works are all two-proof bundles; one proof of one, for
    // the same fee and prover, is a one-proof work with the same sok digest.
    let two = ocaml_works().remove(0);
    let v2::TransactionSnarkWorkTStableV2Proofs::Two((first, _)) = &two.proofs else {
        panic!("a two-proof bundle")
    };
    let one = v2::TransactionSnarkWorkTStableV2 {
        proofs: v2::TransactionSnarkWorkTStableV2Proofs::One(first.clone()),
        ..two.clone()
    };
    let (one, two) = (&one, &two);
    let returned = |work: &v2::TransactionSnarkWorkTStableV2,
                    fee: v2::CurrencyFeeStableV1,
                    statements: Vec<v2::MinaStateSnarkedLedgerStateStableV2>| {
        let jobs = statements.len();
        Outcome::WorkReturned {
            request_id: 1,
            kinds: "test".to_owned(),
            statements,
            proofs: work.proofs.clone(),
            prove_ms: vec![1; jobs],
            round_trip_ms: 2,
            queued_ms: 0,
            checker: Arc::new(WorkChecker {
                fee,
                prover: work.prover.clone(),
                ..base.clone()
            }),
        }
    };
    for work in [one, two] {
        let statements = proof_statements(&work.proofs);
        match work_event(returned(work, work.fee.clone(), statements.clone())) {
            Some(ExternalSnarkWorkerEvent::WorkResult(result)) => {
                assert_eq!(encode(&result.proofs), encode(&work.proofs));
                // With the key and fee the proof was made for (P5.4).
                assert_eq!(result.snarker, work.prover);
                assert_eq!(result.fee, work.fee);
            }
            other => panic!("OCaml's own work must pass: {other:?}"),
        }
        let other_fee = v2::CurrencyFeeStableV1(
            v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1((work.fee.0.as_u64() + 1).into()),
        );
        assert!(matches!(
            work_event(returned(work, other_fee, statements.clone())),
            Some(ExternalSnarkWorkerEvent::WorkError(ExternalSnarkWorkerWorkError::Error(e)))
                if e.contains("own proof failed verification")
        ));
        let mut other_statements = statements;
        let supply = &mut other_statements[0].0.supply_increase.magnitude;
        *supply = v2::CurrencyAmountStableV1((supply.0.as_u64() + 1).into());
        assert!(matches!(
            work_event(returned(work, work.fee.clone(), other_statements)),
            Some(ExternalSnarkWorkerEvent::WorkError(ExternalSnarkWorkerWorkError::Error(e)))
                if e.contains("other statements")
        ));
    }
}

/// A worker that is not told to stop proves on: a worker event for every
/// worker outcome, none for block outcomes.
#[test]
fn every_worker_outcome_becomes_its_event() {
    let cases = [
        (Outcome::WorkerStarted, "Started"),
        (Outcome::WorkerKilled, "Killed"),
        (
            Outcome::WorkerFailed {
                error: "x".to_owned(),
            },
            "Error",
        ),
        (Outcome::WorkCancelled { request_id: 1 }, "WorkCancelled"),
        (
            Outcome::WorkRejected {
                request_id: 1,
                error: "x".to_owned(),
            },
            "WorkError",
        ),
    ];
    for (outcome, expected) in cases {
        let event = work_event(outcome).unwrap();
        let name = match event {
            ExternalSnarkWorkerEvent::Started => "Started",
            ExternalSnarkWorkerEvent::Killed => "Killed",
            ExternalSnarkWorkerEvent::Error(_) => "Error",
            ExternalSnarkWorkerEvent::WorkCancelled => "WorkCancelled",
            ExternalSnarkWorkerEvent::WorkError(_) => "WorkError",
            ExternalSnarkWorkerEvent::WorkResult(_) => "WorkResult",
        };
        assert_eq!(name, expected);
    }
    assert!(work_event(Outcome::Rejected {
        block_hash: hash(1),
        request_id: 1,
        error: "x".to_owned(),
    })
    .is_none());
}
