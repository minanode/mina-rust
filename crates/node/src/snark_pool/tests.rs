//! Protocol fixtures supply the proof data. Verification outcomes here are
//! injected at the service boundary to test admission, not cryptography.
use super::{candidate::SnarkPoolCandidateAction, *};
use crate::{
    p2p::{channels::rpc::*, PeerId},
    State,
};
use ledger::scan_state::scan_state::{transaction_snark::OneOrTwo, AvailableJobMessage};
use mina_core::{consensus::ConsensusConstants, constants, snark::Snark};
use mina_p2p_messages::{
    rpc::{CompletedSnarks, GetCompletedSnarksV1},
    rpc_kernel::PayloadBinprotReader,
    v2,
};
use mina_snark::{
    work_verify::{SnarkWorkVerifyAction, SnarkWorkVerifyError, SnarkWorkVerifyId},
    BlockVerifier, SnarkAction, TransactionVerifier,
};
use redux::{EnablingCondition, Timestamp};

#[derive(Clone)]
struct Service {
    now: redux::Instant,
    batches: Vec<(SnarkWorkVerifyId, Vec<Snark>)>,
    account: Result<ProverAccount, String>,
    delay_accounts: bool,
    account_requests: Vec<crate::ledger::read::LedgerReadId>,
    /// What the worker's service was asked to do (P5.4).
    worker_effects: Vec<String>,
    /// The jobs the worker's random selection chose among, and how many it
    /// asked for (SW-rand).
    random_choices: Vec<(Vec<mina_core::snark::SnarkJobId>, usize)>,
}
impl redux::Service for Service {}
impl redux::TimeService for Service {
    fn monotonic_time(&mut self) -> redux::Instant {
        self.now
    }
}
fn effects(store: &mut crate::Store<Service>, action: crate::ActionWithMeta) {
    if let crate::Action::ExternalSnarkWorkerEffects(effect) = action.action() {
        use crate::external_snark_worker_effectful::ExternalSnarkWorkerEffectfulAction as Effect;
        let effect = match effect {
            Effect::Start { fee, .. } => format!("start {}", fee.0.as_u64()),
            Effect::Kill => "kill".to_owned(),
            Effect::SaveSetting { setting: Some(s) } => format!("save {}", s.fee),
            Effect::SaveSetting { setting: None } => "save off".to_owned(),
            Effect::SubmitWork { .. } => "submit".to_owned(),
            Effect::CancelWork => "cancel".to_owned(),
        };
        store.service.worker_effects.push(effect);
    }
    // The worker's random selection (SW-rand): the choices are kept, and the
    // service chooses the last in scan-state order, the job `seq` takes last.
    if let crate::Action::SnarkPoolEffect(SnarkPoolEffectfulAction::SnarkPoolJobsRandomChoose {
        choices,
        count,
        on_result,
    }) = action.action()
    {
        store.service.random_choices.push((choices.clone(), *count));
        let pool = &store.state().snark_pool;
        let mut chosen = choices.clone();
        chosen.sort_by_key(|id| std::cmp::Reverse(pool.get(id).map(|job| job.order)));
        chosen.truncate(*count);
        store.dispatch_callback(on_result.clone(), chosen);
    }
    if let crate::Action::LedgerEffects(
        crate::ledger_effectful::LedgerEffectfulAction::ReadInit {
            id,
            request: request @ crate::ledger::read::LedgerReadRequest::SnarkPoolProver(_),
            ..
        },
    ) = action.action()
    {
        let id = *id;
        store.service.account_requests.push(id);
        store.dispatch(crate::ledger::read::LedgerReadAction::Pending {
            id,
            request: request.clone(),
        });
        if !store.service.delay_accounts {
            store.dispatch(crate::ledger::read::LedgerReadAction::Success {
                id,
                response: crate::ledger::read::LedgerReadResponse::SnarkPoolProver(
                    store.service.account.clone(),
                ),
            });
        }
    }
    if let crate::Action::Snark(SnarkAction::WorkVerifyEffect(
        mina_snark::work_verify_effectful::SnarkWorkVerifyEffectfulAction::Init {
            req_id,
            batch,
            ..
        },
    )) = action.action()
    {
        store.service.batches.push((*req_id, batch.clone()));
    }
}
fn state() -> State {
    let constants = ConsensusConstants::create(
        constants::constraint_constants(),
        &constants::PROTOCOL_CONSTANTS,
    );
    let config = crate::Config {
        ledger: crate::config::LedgerConfig {},
        snark: crate::config::SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: mina_snark::get_srs(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: mina_snark::get_srs(),
        },
        p2p: crate::config::P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: crate::p2p::identity::SecretKey::deterministic(0).public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: Default::default(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            meshsub: Default::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: crate::config::TransitionFrontierConfig::new(
            crate::config::DEVNET_CONFIG.clone(),
        ),
        archive: None,
        block_producer: None,
        global: crate::config::GlobalConfig {
            build: Box::new(crate::config::BuildEnv::get()),
            snarker: None,
            consensus_constants: constants.clone(),
            client_port: None,
            testing_run: true,
            chain_id_override: None,
            skip_proof_verification: false,
            worker_control: false,
            // OCaml's `seq`, for a known order; the selection tests set it.
            snarker_strategy: crate::SnarkerStrategy::Sequential,
            block_first_lead: None,
        },
        tx_pool: ledger::transaction_pool::Config {
            trust_system: (),
            pool_max_size: 10,
            slot_tx_end: None,
        },
    };
    let mut state = State::new(config, &constants, Timestamp::ZERO);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    state.transition_frontier.sync =
        crate::transition_frontier::sync::TransitionFrontierSyncState::Synced {
            time: Timestamp::ZERO,
        };
    let block = mina_p2p_messages::rpc::GetBestTipV2::response_payload(
        &mut include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin")
            .as_slice(),
    )
    .unwrap()
    .unwrap()
    .data;
    let block = std::sync::Arc::new(block);
    state
        .transition_frontier
        .best_chain
        .push(mina_core::block::AppliedBlock {
            block: mina_core::block::BlockWithHash::try_new(block).unwrap(),
            just_emitted_a_proof: false,
        });
    state.snark_pool.jobs_tip = state
        .transition_frontier
        .best_tip()
        .map(|tip| tip.hash().clone());
    state
}
fn works() -> Vec<v2::TransactionSnarkWorkTStableV2> {
    let bytes =
        include_bytes!("../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin");
    GetCompletedSnarksV1::response_payload(&mut bytes.as_slice())
        .unwrap()
        .unwrap()
        .0
        .into_iter()
        .collect()
}
fn job_for_statement(
    statement: &v2::TransactionSnarkWorkStatementStableV2,
    prover: &v2::NonZeroCurvePoint,
) -> OneOrTwo<AvailableJobMessage> {
    // Only statement matching is under test. The requested statements are copied
    // from captured OCaml work; the unused base-job witness is synthetic.
    let base = |statement: &v2::MinaStateSnarkedLedgerStateStableV2| {
        let sparse = v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: mina_p2p_messages::list::List::new(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(v2::LedgerHash::zero()),
        };
        AvailableJobMessage::Base(v2::TransactionSnarkScanStateTransactionWithWitnessStableV2 {
            transaction_with_status: v2::TransactionSnarkScanStateTransactionWithWitnessStableV2TransactionWithStatus {
                data: v2::MinaTransactionTransactionStableV2::Coinbase(v2::MinaBaseCoinbaseStableV1 {
                    receiver: prover.clone(),
                    amount: v2::CurrencyAmountStableV1(0u64.into()),
                    fee_transfer: None,
                }),
                status: v2::MinaBaseTransactionStatusStableV2::Applied,
            },
            state_hash: (v2::StateHash::zero(), v2::MinaBaseStateBodyHashStableV1(mina_p2p_messages::bigint::BigInt::zero()).into()),
            statement: statement.clone(),
            init_stack: statement.source.pending_coinbase_stack.clone(),
            first_pass_ledger_witness: sparse.clone(),
            second_pass_ledger_witness: sparse,
            block_global_slot: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(0u32.into()),
        })
    };
    match statement {
        v2::TransactionSnarkWorkStatementStableV2::One(s) => OneOrTwo::One(base(s)),
        v2::TransactionSnarkWorkStatementStableV2::Two((a, b)) => OneOrTwo::Two((base(a), base(b))),
    }
}
fn store_with_job(work: &v2::TransactionSnarkWorkTStableV2) -> crate::Store<Service> {
    let snark = Snark::from(work.clone());
    let mut store = store_with_requested_job(job_for_statement(&snark.statement(), &snark.snarker));
    let tip = store
        .state()
        .transition_frontier
        .best_tip()
        .unwrap()
        .hash()
        .clone();
    edit_state(&mut store, |state| {
        state.snark_pool.prover_accounts.insert(
            snark.snarker.into(),
            (tip, ProverAccount::Present { can_receive: true }),
        );
    });
    store
}
fn store_with_requested_job(job: OneOrTwo<AvailableJobMessage>) -> crate::Store<Service> {
    let mut state = state();
    state.snark_pool.insert(JobState {
        time: Timestamp::ZERO,
        id: (&job).into(),
        job,
        commitment: None,
        snark: None,
        order: 0,
    });
    redux::Store::new(
        crate::reducer,
        effects,
        Service {
            now: redux::Instant::now(),
            batches: Vec::new(),
            account: Ok(ProverAccount::Present { can_receive: true }),
            delay_accounts: false,
            account_requests: Vec::new(),
            worker_effects: Vec::new(),
            random_choices: Vec::new(),
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    )
}

// Rebuild only the test store to model a frontier/ledger event while keeping
// pending proof/read requests and the real reducers intact.
fn edit_state(store: &mut crate::Store<Service>, edit: impl FnOnce(&mut State)) {
    let mut state = store.state().clone();
    edit(&mut state);
    *store = redux::Store::new(
        crate::reducer,
        effects,
        store.service.clone(),
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
}

fn one_and_two_work() -> Vec<Snark> {
    use mina_p2p_messages::binprot::BinProtRead;
    let one = v2::TransactionSnarkWorkTStableV2::binprot_read(
        &mut include_bytes!("../../../ledger/src/verifier/fixtures/mainnet-556667-40.bin")
            .as_slice(),
    )
    .unwrap();
    assert!(matches!(
        one.proofs,
        v2::TransactionSnarkWorkTStableV2Proofs::One(_)
    ));
    let two = works()
        .into_iter()
        .find(|w| matches!(w.proofs, v2::TransactionSnarkWorkTStableV2Proofs::Two(_)))
        .unwrap();
    vec![one.into(), two.into()]
}

// OCaml's equality compares the full statements and One_or_two shape. Change
// each omitted field category in the requested job, leaving the peer's genuine
// proof untouched: it is valid work, but for a different local statement.
fn colliding_statements(work: &Snark) -> Vec<v2::TransactionSnarkWorkStatementStableV2> {
    type Statement = v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement;
    let changes: &[fn(&mut Statement)] = &[
        |s| s.connecting_ledger_left = v2::LedgerHash::zero(),
        |s| s.connecting_ledger_right = v2::LedgerHash::zero(),
        |s| s.supply_increase.magnitude.0 = (s.supply_increase.magnitude.0.as_u64() + 1).into(),
        |s| {
            s.fee_excess.0.amount.magnitude.0 =
                (s.fee_excess.0.amount.magnitude.0.as_u64() + 1).into()
        },
        |s| s.source.pending_coinbase_stack.state.curr = v2::CoinbaseStackHash::zero(),
        |s| s.target.pending_coinbase_stack.state.curr = v2::CoinbaseStackHash::zero(),
        |s| s.source.local_state.success = !s.source.local_state.success,
        |s| s.target.local_state.will_succeed = !s.target.local_state.will_succeed,
    ];
    let original = work.statement();
    let count = match &original {
        v2::TransactionSnarkWorkStatementStableV2::One(_) => 1,
        v2::TransactionSnarkWorkStatementStableV2::Two(_) => 2,
    };
    let mut variants = Vec::new();
    for index in 0..count {
        for change in changes {
            let mut stmt = original.clone();
            let part = match &mut stmt {
                v2::TransactionSnarkWorkStatementStableV2::One(a) => a,
                v2::TransactionSnarkWorkStatementStableV2::Two((a, b)) => {
                    if index == 0 {
                        a
                    } else {
                        b
                    }
                }
            };
            change(&mut part.0);
            assert_ne!(stmt, original);
            variants.push(stmt);
        }
    }
    if let v2::TransactionSnarkWorkStatementStableV2::Two((a, b)) = &original {
        // The intermediate ledgers are also omitted from the outer job ID.
        for first_pass in [false, true] {
            let (mut a, b) = (a.clone(), b.clone());
            if first_pass {
                a.0.target.first_pass_ledger = v2::LedgerHash::zero();
            } else {
                a.0.target.second_pass_ledger = v2::LedgerHash::zero();
            }
            let stmt = v2::TransactionSnarkWorkStatementStableV2::Two((a, b));
            assert_ne!(stmt, original);
            variants.push(stmt);
        }
    }
    variants.push(match original {
        v2::TransactionSnarkWorkStatementStableV2::One(a) => {
            v2::TransactionSnarkWorkStatementStableV2::Two((a.clone(), a))
        }
        v2::TransactionSnarkWorkStatementStableV2::Two((mut a, b)) => {
            a.0.target = b.0.target;
            v2::TransactionSnarkWorkStatementStableV2::One(a)
        }
    });
    variants
}

#[test]
fn gossip_and_pulled_work_require_full_statements_and_proof_count() {
    let peer_id = PeerId::from_bytes([4; 32]);
    for work in one_and_two_work() {
        let mut store = store_with_job(&work.to_wire().unwrap());
        for pulled in [false, true] {
            for statement in colliding_statements(&work) {
                let job = job_for_statement(&statement, &work.snarker);
                assert_eq!(mina_core::snark::SnarkJobId::from(&job), work.job_id());
                store.dispatch(SnarkPoolAction::JobsUpdate {
                    jobs: std::sync::Arc::new(vec![job]),
                    orphaned_snarks: vec![],
                });
                if pulled {
                    store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
                        peer_id,
                        works: CompletedSnarks([work.to_wire().unwrap()].into_iter().collect()),
                    });
                } else {
                    assert!(!store.dispatch(SnarkPoolCandidateAction::WorkFetchSuccess {
                        peer_id,
                        work: work.clone()
                    }));
                    store.dispatch(SnarkPoolCandidateAction::WorkVerifyNext);
                }
                assert!(store
                    .state()
                    .snark_pool
                    .candidates
                    .get(peer_id, &work.job_id())
                    .is_none());
                assert!(store
                    .state()
                    .snark_pool
                    .get(&work.job_id())
                    .unwrap()
                    .snark
                    .is_none());
                assert!(store.service.batches.is_empty());
                // Final admission also rejects it, even with an injected success.
                assert!(!store.dispatch(SnarkPoolAction::WorkAdd {
                    snark: work.clone(),
                    sender: peer_id,
                    is_sender_local: false
                }));
            }
        }
    }
}

#[test]
fn matching_gossip_and_pulled_work_are_admitted_only_after_verification() {
    let peer_id = PeerId::from_bytes([4; 32]);
    for work in one_and_two_work() {
        for pulled in [false, true] {
            let mut store = store_with_job(&work.to_wire().unwrap());
            if pulled {
                store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
                    peer_id,
                    works: CompletedSnarks([work.to_wire().unwrap()].into_iter().collect()),
                });
            } else {
                assert!(store.dispatch(SnarkPoolCandidateAction::WorkFetchSuccess {
                    peer_id,
                    work: work.clone()
                }));
                store.dispatch(SnarkPoolCandidateAction::WorkVerifyNext);
            }
            assert!(store
                .state()
                .snark_pool
                .get(&work.job_id())
                .unwrap()
                .snark
                .is_none());
            assert_eq!(store.service.batches.len(), 1);
            let req_id = store.service.batches[0].0;
            assert!(store.dispatch(SnarkWorkVerifyAction::Success { req_id }));
            assert!(store
                .state()
                .snark_pool
                .get(&work.job_id())
                .unwrap()
                .snark
                .is_some());
        }
    }
}

#[test]
fn producer_lookup_rejects_same_id_different_statement_or_proof_count() {
    use ledger::scan_state::scan_state::transaction_snark::{work, Statement};
    let to_statement = |statement: &v2::TransactionSnarkWorkStatementStableV2| match statement {
        v2::TransactionSnarkWorkStatementStableV2::One(s) => {
            OneOrTwo::One(Statement::<()>::try_from(&s.0).unwrap())
        }
        v2::TransactionSnarkWorkStatementStableV2::Two((a, b)) => OneOrTwo::Two((
            Statement::<()>::try_from(&a.0).unwrap(),
            Statement::<()>::try_from(&b.0).unwrap(),
        )),
    };
    for snark in one_and_two_work() {
        let pool = [(snark.job_id(), snark.clone())].into_iter().collect();
        for statement in colliding_statements(&snark) {
            let requested: work::Statement = to_statement(&statement);
            assert_eq!(
                mina_core::snark::SnarkJobId::from(&requested),
                snark.job_id()
            );
            assert!(crate::ledger::completed_work_for_statement(&pool, &requested).is_none());
        }
        let requested = to_statement(&snark.statement());
        let selected = crate::ledger::completed_work_for_statement(&pool, &requested).unwrap();
        assert_eq!(selected.statement(), requested);
    }
}

#[test]
fn job_change_with_same_id_discards_verified_and_pending_work() {
    let peer_id = PeerId::from_bytes([4; 32]);
    let work = one_and_two_work().remove(0);
    for finish_before_change in [false, true] {
        let mut store = store_with_job(&work.to_wire().unwrap());
        store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
            peer_id,
            works: CompletedSnarks([work.to_wire().unwrap()].into_iter().collect()),
        });
        let req_id = store.service.batches[0].0;
        if finish_before_change {
            store.dispatch(SnarkWorkVerifyAction::Success { req_id });
        }
        let replacement = job_for_statement(&colliding_statements(&work).remove(0), &work.snarker);
        store.dispatch(SnarkPoolAction::JobsUpdate {
            jobs: std::sync::Arc::new(vec![replacement]),
            orphaned_snarks: vec![SnarkWork {
                work: work.clone(),
                received_t: Timestamp::ZERO,
                sender: peer_id,
            }],
        });
        if !finish_before_change {
            store.dispatch(SnarkWorkVerifyAction::Success { req_id });
        }
        assert!(store
            .state()
            .snark_pool
            .get(&work.job_id())
            .unwrap()
            .snark
            .is_none());
        assert!(store
            .state()
            .snark_pool
            .candidates
            .get(peer_id, &work.job_id())
            .is_none());
    }
}

#[test]
fn merge_job_matching_uses_both_child_statements_and_fails_closed() {
    for work in one_and_two_work() {
        // OCaml Statement.merge(identity at source, statement) = statement.
        // The expected statement and right child come from OCaml's fixture.
        // The identity child is synthetic; this tests matching, not its proof.
        let merge = |right: &v2::LedgerProofProdStableV2| {
            let mut left = right.clone();
            let s = &mut left.0.statement;
            s.target = s.source.clone();
            s.connecting_ledger_right = s.connecting_ledger_left.clone();
            s.supply_increase.magnitude.0 = 0u64.into();
            s.supply_increase.sgn = v2::SgnStableV1::Pos;
            s.fee_excess.0.amount.magnitude.0 = 0u64.into();
            s.fee_excess.1.amount.magnitude.0 = 0u64.into();
            AvailableJobMessage::Merge {
                left,
                right: right.clone(),
            }
        };
        let job = match work.proofs.get().unwrap().as_ref() {
            v2::TransactionSnarkWorkTStableV2Proofs::One(p) => OneOrTwo::One(merge(p)),
            v2::TransactionSnarkWorkTStableV2Proofs::Two((a, b)) => {
                OneOrTwo::Two((merge(a), merge(b)))
            }
        };
        let mut job_state = store_with_requested_job(job)
            .state()
            .snark_pool
            .get(&work.job_id())
            .unwrap()
            .clone();
        assert!(job_state.matches_work(&work));
        let first = match &mut job_state.job {
            OneOrTwo::One(a) | OneOrTwo::Two((a, _)) => a,
        };
        let AvailableJobMessage::Merge { left, .. } = first else {
            unreachable!()
        };
        left.0.statement.supply_increase.magnitude.0 = 1u64.into();
        assert!(!job_state.matches_work(&work));
        let first = match &mut job_state.job {
            OneOrTwo::One(a) | OneOrTwo::Two((a, _)) => a,
        };
        let AvailableJobMessage::Merge { left, .. } = first else {
            unreachable!()
        };
        left.0.statement.target.first_pass_ledger = v2::LedgerHash::zero();
        assert!(super::snark_pool_state::job_statement(&job_state.job).is_none());
        assert!(!job_state.matches_work(&work));
    }
}
#[test]
fn fetched_work_is_unusable_until_verification_and_rejection_never_inserts_it() {
    let work = works().remove(0);
    let job = Snark::from(work.clone()).job_id();
    let peer_id = PeerId::from_bytes([4; 32]);
    for valid in [false, true] {
        let mut store = store_with_job(&work);
        assert!(store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
            peer_id,
            works: CompletedSnarks(std::iter::once(work.clone()).collect())
        }));
        assert!(store.state().snark_pool.get(&job).unwrap().snark.is_none());
        assert_eq!(store.service.batches.len(), 1);
        let (req_id, batch) = &store.service.batches[0];
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].job_id(), job);
        let req_id = *req_id;
        if valid {
            assert!(store.dispatch(SnarkWorkVerifyAction::Success { req_id }));
        } else {
            assert!(store.dispatch(SnarkWorkVerifyAction::Error {
                req_id,
                error: SnarkWorkVerifyError::VerificationFailed
            }));
        }
        assert_eq!(
            store.state().snark_pool.get(&job).unwrap().snark.is_some(),
            valid
        );
        // A late verifier success after a failure/finish cannot admit work.
        assert!(!store.dispatch(SnarkWorkVerifyAction::Success { req_id }));
    }
}
#[test]
fn irrelevant_and_duplicate_pulled_work_do_not_trigger_verification() {
    let work = works().remove(0);
    let peer_id = PeerId::from_bytes([4; 32]);
    let mut store = store_with_job(&work);
    let received = SnarkPoolAction::CompletedSnarksReceived {
        peer_id,
        works: CompletedSnarks([work.clone(), work.clone()].into_iter().collect()),
    };
    assert!(store.dispatch(received.clone()));
    assert_eq!(store.service.batches[0].1.len(), 1);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });
    store.dispatch(received);
    assert_eq!(store.service.batches.len(), 1);
    store.dispatch(SnarkPoolAction::JobsUpdate {
        jobs: std::sync::Arc::new(Vec::new()),
        orphaned_snarks: Vec::new(),
    });
    store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
        peer_id,
        works: CompletedSnarks([work].into_iter().collect()),
    });
    assert_eq!(store.service.batches.len(), 1);
}
#[test]
fn request_action_cannot_bypass_budget_when_peer_reconnects() {
    use crate::p2p::{P2pPeerState, P2pPeerStatus, P2pPeerStatusReady};
    let mut state = state();
    let peer_id = PeerId::from_bytes([5; 32]);
    let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
    ready.channels.rpc = P2pChannelsRpcState::Ready {
        time: Timestamp::ZERO,
        local: P2pRpcLocalState::WaitingForRequest {
            time: Timestamp::ZERO,
        },
        remote: P2pRpcRemoteState {
            pending_requests: Default::default(),
            last_responded: Timestamp::ZERO,
        },
    };
    let peer = P2pPeerState {
        is_libp2p: true,
        dial_opts: None,
        identify: None,
        status: P2pPeerStatus::Ready(Box::new(ready)),
    };
    let p2p = state.p2p.ready_mut().unwrap();
    p2p.peers.insert(peer_id, peer.clone());
    let action = P2pChannelsRpcAction::RequestSend {
        peer_id,
        id: 0,
        request: Box::new(P2pRpcRequest::CompletedSnarks),
        on_init: None,
    };
    assert!(action.is_enabled(p2p, Timestamp::ZERO));
    p2p.completed_snarks_requests
        .record(peer_id, None, Timestamp::ZERO);
    p2p.peers.remove(&peer_id);
    p2p.peers.insert(peer_id, peer);
    assert!(!action.is_enabled(p2p, Timestamp::new(299_000_000_000)));
    assert!(action.is_enabled(p2p, Timestamp::new(300_000_000_000)));
}
#[test]
fn coverage_distinguishes_required_prefix_from_total_pool_size() {
    use super::work_coverage::coverage_counts;
    // create_diff stops at the first missing job (OCaml staged_ledger.ml).
    assert_eq!(
        coverage_counts(&[0, 1, 2, 3], |j| [0, 2, 3, 99].contains(j)),
        (3, 1)
    );
    assert_eq!(
        coverage_counts(&[0, 1, 2, 3], |j| [1, 2, 3].contains(j)),
        (3, 0)
    );
    assert_eq!(coverage_counts(&[0, 1, 2, 3], |_| true), (4, 4));
    assert_eq!(coverage_counts::<u8>(&[], |_| false), (0, 0));
}

// These expected admission outcomes are the predicates in OCaml 4.0.2
// snark_pool.ml:164-165,370-424, not values generated by our implementation.
#[test]
fn prover_account_creation_fee_boundaries() {
    let creation = 1_000_000_000;
    for fee in [0, creation - 1] {
        assert_eq!(
            check_prover(fee, creation, ProverAccount::Missing),
            Err(AdmissionError::InsufficientFee)
        );
    }
    for fee in [creation, creation + 1, u64::MAX] {
        assert_eq!(check_prover(fee, creation, ProverAccount::Missing), Ok(()));
    }
    assert_eq!(check_prover(0, 0, ProverAccount::Missing), Ok(()));
    for fee in [0, creation - 1, creation, u64::MAX] {
        assert_eq!(
            check_prover(fee, creation, ProverAccount::Present { can_receive: true }),
            Ok(())
        );
        assert_eq!(
            check_prover(fee, creation, ProverAccount::Present { can_receive: false }),
            Err(AdmissionError::ProverCannotReceive)
        );
    }
}

#[test]
fn both_access_and_receive_must_allow_no_authorization_even_for_zero_fee() {
    use ledger::AuthRequired;
    let work = one_and_two_work().remove(0);
    let mut account = ledger::Account::create_with(
        ledger::AccountId::new(
            (&work.snarker).try_into().unwrap(),
            ledger::TokenId::default(),
        ),
        ledger::scan_state::currency::Balance::from_u64(u64::MAX),
    );
    for access in [
        AuthRequired::None,
        AuthRequired::Either,
        AuthRequired::Proof,
        AuthRequired::Signature,
        AuthRequired::Impossible,
        AuthRequired::Both,
    ] {
        for receive in [
            AuthRequired::None,
            AuthRequired::Either,
            AuthRequired::Proof,
            AuthRequired::Signature,
            AuthRequired::Impossible,
            AuthRequired::Both,
        ] {
            account.permissions.access = access;
            account.permissions.receive = receive;
            let expected = access == AuthRequired::None && receive == AuthRequired::None;
            assert_eq!(
                check_prover(
                    0,
                    1_000_000_000,
                    ProverAccount::from_account(Some(&account))
                )
                .is_ok(),
                expected,
                "{access:?}/{receive:?}"
            );
            // The pool does not preflight balance overflow when receiving a fee.
            assert_eq!(
                check_prover(
                    u64::MAX,
                    1_000_000_000,
                    ProverAccount::from_account(Some(&account))
                )
                .is_ok(),
                expected
            );
        }
    }
}

fn receive(store: &mut crate::Store<Service>, work: &Snark, pulled: bool) {
    let peer_id = PeerId::from_bytes([4; 32]);
    if pulled {
        store.dispatch(SnarkPoolAction::CompletedSnarksReceived {
            peer_id,
            works: CompletedSnarks([work.to_wire().unwrap()].into_iter().collect()),
        });
    } else {
        store.dispatch(SnarkPoolAction::WorkCheck {
            work: work.clone(),
            sender: peer_id,
            is_sender_local: false,
            stage: AdmissionStage::Received(WorkSource::Gossip),
            retry: false,
        });
    }
}

#[test]
fn gossip_and_pulls_obey_fee_permission_and_reference_checks_before_verification() {
    for pulled in [false, true] {
        for (account, fee, referenced, expected) in [
            (
                ProverAccount::Missing,
                999_999_999,
                true,
                Some(AdmissionError::InsufficientFee),
            ),
            (ProverAccount::Missing, 1_000_000_000, true, None),
            (
                ProverAccount::Present { can_receive: false },
                0,
                true,
                Some(AdmissionError::ProverCannotReceive),
            ),
            (
                ProverAccount::Present { can_receive: true },
                0,
                false,
                Some(AdmissionError::WorkNotReferenced),
            ),
            (ProverAccount::Present { can_receive: true }, 0, true, None),
        ] {
            let mut work = one_and_two_work().remove(0);
            work.fee = v2::CurrencyFeeStableV1(fee.into());
            let mut store = store_with_job(&work.to_wire().unwrap());
            edit_state(&mut store, |state| state.snark_pool.prover_accounts.clear());
            store.service.account = Ok(account);
            if !referenced {
                edit_state(&mut store, |state| {
                    state.snark_pool.remove(&work.job_id());
                });
            }
            receive(&mut store, &work, pulled);
            assert_eq!(store.service.account_requests.len(), 1);
            assert_eq!(store.service.batches.len(), usize::from(expected.is_none()));
            assert_eq!(store.state().snark_pool.admission_stats.admitted, 0);
            let stats = &store.state().snark_pool.admission_stats;
            assert_eq!(
                stats.insufficient_fee,
                u64::from(expected == Some(AdmissionError::InsufficientFee))
            );
            assert_eq!(
                stats.prover_cannot_receive,
                u64::from(expected == Some(AdmissionError::ProverCannotReceive))
            );
            assert_eq!(
                stats.work_not_referenced,
                u64::from(expected == Some(AdmissionError::WorkNotReferenced))
            );
            if expected.is_none() {
                let req_id = store.service.batches[0].0;
                store.dispatch(SnarkWorkVerifyAction::Success { req_id });
                assert_eq!(store.state().snark_pool.admission_stats.admitted, 1);
                assert_eq!(store.state().snark_pool.admission_stats.forwarded, 1);
            } else {
                assert_eq!(stats.forwarded, 0);
            }
        }
    }
}

#[test]
fn bootstrap_work_is_dropped_and_not_replayed_after_sync() {
    for fee in [0u64, 1_000_000_000] {
        let mut work = one_and_two_work().remove(0);
        work.fee = v2::CurrencyFeeStableV1(fee.into());
        for pulled in [false, true] {
            let mut store = store_with_job(&work.to_wire().unwrap());
            edit_state(&mut store, |state| {
                state.transition_frontier.best_chain.clear()
            });
            receive(&mut store, &work, pulled);
            assert!(store.service.batches.is_empty());
            assert!(store.service.account_requests.is_empty());
            assert_eq!(
                store.state().snark_pool.admission_stats.insufficient_fee,
                u64::from(fee == 0)
            );
            assert_eq!(
                store.state().snark_pool.admission_stats.no_frontier,
                u64::from(fee != 0)
            );
            assert_eq!(store.state().snark_pool.candidates.check().0, 0);
        }
    }
}

fn advance_tip(store: &mut crate::Store<Service>, jobs: Vec<OneOrTwo<AvailableJobMessage>>) {
    let mut block = store
        .state()
        .transition_frontier
        .best_tip()
        .unwrap()
        .block
        .as_ref()
        .clone();
    block
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = (block
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length
        .as_u32()
        + 1)
    .into();
    let block = mina_core::block::BlockWithHash::try_new(std::sync::Arc::new(block)).unwrap();
    edit_state(store, |state| {
        state.transition_frontier.best_chain = vec![mina_core::block::AppliedBlock {
            block,
            just_emitted_a_proof: false,
        }];
    });
    store.dispatch(SnarkPoolAction::JobsUpdate {
        jobs: std::sync::Arc::new(jobs),
        orphaned_snarks: vec![],
    });
}

#[test]
fn a_tip_change_during_proof_verification_rechecks_prover_and_work() {
    for remove_work in [false, true] {
        let work = one_and_two_work().remove(0);
        let mut store = store_with_job(&work.to_wire().unwrap());
        receive(&mut store, &work, false);
        let req_id = store.service.batches[0].0;
        let jobs = if remove_work {
            vec![]
        } else {
            vec![job_for_statement(&work.statement(), &work.snarker)]
        };
        advance_tip(&mut store, jobs);
        store.service.account = Ok(ProverAccount::Present { can_receive: false });
        store.dispatch(SnarkWorkVerifyAction::Success { req_id });
        assert_eq!(store.state().snark_pool.admission_stats.admitted, 0);
        assert_eq!(store.state().snark_pool.admission_stats.forwarded, 0);
        assert_eq!(
            store
                .state()
                .snark_pool
                .admission_stats
                .rejected_after_verification,
            1
        );
    }
}

#[test]
fn stale_account_results_cannot_authorize_a_new_tip() {
    let work = one_and_two_work().remove(0);
    let mut store = store_with_job(&work.to_wire().unwrap());
    edit_state(&mut store, |state| state.snark_pool.prover_accounts.clear());
    store.service.delay_accounts = true;
    receive(&mut store, &work, false);
    let id = store.service.account_requests[0];
    advance_tip(
        &mut store,
        vec![job_for_statement(&work.statement(), &work.snarker)],
    );
    store.service.account = Ok(ProverAccount::Present { can_receive: false });
    store.service.delay_accounts = false;
    store.dispatch(crate::ledger::read::LedgerReadAction::Success {
        id,
        response: crate::ledger::read::LedgerReadResponse::SnarkPoolProver(Ok(
            ProverAccount::Present { can_receive: true },
        )),
    });
    assert_eq!(store.service.account_requests.len(), 2);
    assert!(store.service.batches.is_empty());
    assert_eq!(store.state().snark_pool.admission_stats.received_gossip, 1);
    assert_eq!(
        store
            .state()
            .snark_pool
            .admission_stats
            .stale_account_results,
        1
    );
    assert_eq!(
        store
            .state()
            .snark_pool
            .admission_stats
            .prover_cannot_receive,
        1
    );
}

#[test]
fn retained_work_cannot_be_sent_before_new_tip_account_revalidation() {
    let work = one_and_two_work().remove(0);
    let mut store = store_with_job(&work.to_wire().unwrap());
    receive(&mut store, &work, false);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });
    let broadcast = SnarkPoolAction::WorkBroadcast {
        snark: work.clone(),
        is_local: false,
    };
    assert!(broadcast.is_enabled(store.state(), Timestamp::ZERO));
    store.service.delay_accounts = true;
    advance_tip(
        &mut store,
        vec![job_for_statement(&work.statement(), &work.snarker)],
    );
    assert!(!broadcast.is_enabled(store.state(), Timestamp::ZERO));
    assert_eq!(store.state().snark_pool.completed_snarks_iter().count(), 0);
    assert!(store
        .state()
        .snark_pool
        .next_snarks_to_send((0, 10))
        .0
        .is_empty());
    let id = *store.service.account_requests.last().unwrap();
    store.dispatch(crate::ledger::read::LedgerReadAction::Success {
        id,
        response: crate::ledger::read::LedgerReadResponse::SnarkPoolProver(Ok(
            ProverAccount::Present { can_receive: false },
        )),
    });
    assert!(store
        .state()
        .snark_pool
        .get(&work.job_id())
        .unwrap()
        .snark
        .is_none());
    assert_eq!(
        store
            .state()
            .snark_pool
            .admission_stats
            .revalidation_removed,
        1
    );
    assert_eq!(store.state().snark_pool.admission_stats.forwarded, 1);
}

#[test]
fn missing_ledger_is_not_a_missing_account_and_equal_fees_do_not_replace() {
    let work = one_and_two_work().remove(0);
    let mut store = store_with_job(&work.to_wire().unwrap());
    edit_state(&mut store, |state| state.snark_pool.prover_accounts.clear());
    store.service.account = Err("best-tip staged ledger unavailable".into());
    receive(&mut store, &work, false);
    assert!(store.service.batches.is_empty());
    assert_eq!(
        store.state().snark_pool.admission_stats.account_unavailable,
        1
    );
    store.service.account = Ok(ProverAccount::Present { can_receive: true });
    receive(&mut store, &work, false);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });
    receive(&mut store, &work, true);
    assert_eq!(store.service.batches.len(), 1);
    assert_eq!(store.state().snark_pool.admission_stats.not_cheaper, 1);
}

fn cache_churn_work() -> (Snark, Vec<crate::account::AccountPublicKey>) {
    let mut provers = (0..1026)
        .map(|i| crate::account::AccountSecretKey::deterministic(i).public_key())
        .collect::<Vec<_>>();
    provers.sort();
    let mut work = one_and_two_work().remove(0);
    // Account and proof outcomes are injected. The lowest key makes the old
    // pop_first eviction deterministically discard the stored work's account.
    work.snarker = provers.remove(0).into();
    work.fee = v2::CurrencyFeeStableV1(100u64.into());
    (work, provers)
}

fn assert_producer_has_work(store: &crate::Store<Service>, work: &Snark) {
    let mut completed = store.state().snark_pool.completed_snarks_iter();
    let selected = completed.next().expect("stored work must remain usable");
    assert_eq!(selected.snarker, work.snarker);
    assert_eq!(selected.fee, work.fee);
    assert_eq!(selected.proofs, work.proofs);
    // PC1: the pool keeps the proofs in the proof cache, and the producer
    // gets the same work back from it.
    assert!(selected.proofs.is_cached());
    type Work = ledger::scan_state::scan_state::transaction_snark::work::Work;
    assert_eq!(
        Work::try_from(selected).unwrap(),
        Work::try_from(work).unwrap()
    );
    assert_eq!(selected.to_wire().unwrap(), work.to_wire().unwrap());
    assert!(completed.next().is_none());
}

#[test]
fn rejected_provers_cannot_evict_stored_work_or_trigger_account_reads() {
    let (work, provers) = cache_churn_work();
    let mut store = store_with_job(&work.to_wire().unwrap());
    receive(&mut store, &work, false);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });
    assert_producer_has_work(&store, &work);

    // OCaml rejects equal and higher competing fees before checking accounts.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L138-L170>
    for (i, prover) in provers.into_iter().enumerate() {
        let mut candidate = work.clone();
        candidate.snarker = prover.into();
        candidate.fee = v2::CurrencyFeeStableV1((100 + (i % 2) as u64).into());
        receive(&mut store, &candidate, i % 4 < 2);
    }

    assert_producer_has_work(&store, &work);
    assert!(store.service.account_requests.is_empty());
    assert_eq!(store.service.batches.len(), 1);
    assert_eq!(store.state().snark_pool.prover_accounts.len(), 1);
    assert_eq!(store.state().snark_pool.admission_stats.not_cheaper, 1025);
    assert_eq!(store.state().snark_pool.admission_stats.admitted, 1);
    assert_eq!(store.state().snark_pool.admission_stats.forwarded, 1);
}

/// PC1 review fix: a stored work whose cached proof file is swapped for
/// another proof that still decodes, of the same length, is never used. The
/// read fails on the digest; from then on the producer, serving and gossip
/// skip the work, and the pool's periodic check drops it, so its job is open.
#[test]
fn a_swapped_cached_proof_is_skipped_then_dropped() {
    use mina_core::snark::proof_cache::ProofStore;
    use mina_p2p_messages::binprot::{BinProtRead, BinProtWrite};
    type Work = ledger::scan_state::scan_state::transaction_snark::work::Work;

    let (work, _) = cache_churn_work();
    let mut store = store_with_job(&work.to_wire().unwrap());
    receive(&mut store, &work, false);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });
    assert_producer_has_work(&store, &work);

    // Keep the stored work's proofs in files, as a native node does.
    let dir = std::env::temp_dir().join(format!("mina-pc1-pool-swap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = ProofStore::dir(&dir).unwrap();
    edit_state(&mut store, |state| {
        let mut stored = state.snark_pool.get(&work.job_id()).unwrap().snark.clone();
        let stored = stored.as_mut().unwrap();
        stored.work.proofs = work.proofs.cache_in(&files);
        state.snark_pool.set_snark_work(stored.clone());
    });
    assert_producer_has_work(&store, &work);

    // Swap each file for another valid proof of the same length.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        let mut proof = v2::TransactionSnarkProofStableV2::binprot_read(&mut &bytes[..]).unwrap();
        proof.0.proof.ft_eval1 = proof.0.proof.bulletproof.z_1.clone();
        let mut swapped = Vec::new();
        proof.binprot_write(&mut swapped).unwrap();
        assert_eq!(swapped.len(), bytes.len());
        assert_ne!(swapped, bytes);
        std::fs::write(&path, swapped).unwrap();
    }

    // The producer's read fails: the work is not included, and from then on
    // it is not offered, served or gossiped either.
    let pool = &store.state().snark_pool;
    let selected = pool.completed_snarks_iter().next().unwrap();
    assert!(Work::try_from(selected).is_err());
    assert!(selected.to_wire().is_err());
    assert!(pool.completed_snarks_iter().next().is_none());
    assert!(pool.completed_snarks_for_rpc().unwrap().is_empty());
    assert_eq!(pool.next_snarks_to_send((0, 255)).0.len(), 0);

    // The periodic check drops it, and the job is open again. Enabling
    // conditions see the last action's time: move it on first.
    store.service.now += std::time::Duration::from_secs(60);
    assert!(store.dispatch(SnarkPoolAction::P2pSendAll));
    assert!(store.dispatch(SnarkPoolAction::CheckTimeouts));
    let job = store.state().snark_pool.get(&work.job_id()).unwrap();
    assert!(job.snark.is_none());
    assert_eq!(files.stats().files, 0);
    assert_eq!(files.stats().unreadable, 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delayed_account_results_recheck_competing_fees_before_caching() {
    let (work, provers) = cache_churn_work();
    for stage in [
        AdmissionStage::Received(WorkSource::Gossip),
        AdmissionStage::Verified,
    ] {
        for result in [
            Ok(ProverAccount::Present { can_receive: true }),
            Ok(ProverAccount::Present { can_receive: false }),
            Err("best-tip staged ledger unavailable".into()),
        ] {
            let mut store = store_with_job(&work.to_wire().unwrap());
            let mut candidate = work.clone();
            candidate.snarker = provers[0].clone().into();
            store.service.delay_accounts = true;
            store.dispatch(SnarkPoolAction::WorkCheck {
                work: candidate,
                sender: PeerId::from_bytes([4; 32]),
                is_sender_local: false,
                stage,
                retry: false,
            });
            let id = store.service.account_requests[0];

            // A competing proof is stored while the candidate's ledger read waits.
            receive(&mut store, &work, false);
            let req_id = store.service.batches[0].0;
            store.dispatch(SnarkWorkVerifyAction::Success { req_id });
            store.dispatch(crate::ledger::read::LedgerReadAction::Success {
                id,
                response: crate::ledger::read::LedgerReadResponse::SnarkPoolProver(result),
            });

            assert_producer_has_work(&store, &work);
            assert_eq!(store.state().snark_pool.prover_accounts.len(), 1);
            assert_eq!(store.service.batches.len(), 1);
            let stats = &store.state().snark_pool.admission_stats;
            assert_eq!(
                stats.not_cheaper,
                u64::from(stage != AdmissionStage::Verified)
            );
            assert_eq!(
                stats.rejected_after_verification,
                u64::from(stage == AdmissionStage::Verified)
            );
            assert_eq!(stats.prover_cannot_receive, 0);
            assert_eq!(stats.account_unavailable, 0);
        }
    }
}

#[test]
fn cheaper_invalid_provers_cannot_evict_stored_work() {
    let (work, provers) = cache_churn_work();
    let mut store = store_with_job(&work.to_wire().unwrap());
    receive(&mut store, &work, false);
    let req_id = store.service.batches[0].0;
    store.dispatch(SnarkWorkVerifyAction::Success { req_id });

    for prover in provers {
        let mut candidate = work.clone();
        candidate.snarker = prover.into();
        candidate.fee = v2::CurrencyFeeStableV1(99u64.into());
        receive(&mut store, &candidate, false);
        // Account eligibility alone must not let an unverified candidate evict
        // the account on which the producer's existing proof depends.
        assert_producer_has_work(&store, &work);
        let req_id = store.service.batches.last().unwrap().0;
        assert!(store.dispatch(SnarkWorkVerifyAction::Error {
            req_id,
            error: SnarkWorkVerifyError::VerificationFailed,
        }));
        // The test service does not run disconnection effects. Model their
        // candidate cleanup before the next delivery from this peer.
        assert!(store.dispatch(SnarkPoolCandidateAction::PeerPrune {
            peer_id: PeerId::from_bytes([4; 32]),
        }));
    }

    assert_producer_has_work(&store, &work);
    assert_eq!(store.service.account_requests.len(), 1025);
    assert_eq!(store.service.batches.len(), 1026);
    assert!(store.state().snark_pool.prover_accounts.len() <= 1024);
    assert_eq!(store.state().snark_pool.admission_stats.admitted, 1);
    assert_eq!(store.state().snark_pool.admission_stats.forwarded, 1);
    assert_eq!(
        store
            .state()
            .snark_pool
            .admission_stats
            .failed_verification_batches,
        1025
    );
    advance_tip(
        &mut store,
        vec![job_for_statement(&work.statement(), &work.snarker)],
    );
    assert_producer_has_work(&store, &work);
}

/// P5.1's fixture of mainnet block 548186's zkApp command, OCaml's own base
/// job: shape `SSPPSNN`, proved as 5 segments (`SS P P SS S`) and 4 merges.
fn zkapp_two_proofs_transaction() -> v2::MinaTransactionTransactionStableV2 {
    use mina_p2p_messages::binprot::BinProtRead;
    let (_, witness) = <(
        v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        v2::TransactionWitnessStableV2,
    )>::binprot_read(
        &mut include_bytes!(
            "../../../ledger/src/proofs/data/mesa_mainnet_zkapp_two_proofs_base_job.bin"
        )
        .as_slice(),
    )
    .unwrap();
    witness.transaction
}

/// A job for `work`'s statement at `order`, its first base job carrying
/// `transaction` if given.
fn job_state(
    work: &Snark,
    order: usize,
    transaction: Option<v2::MinaTransactionTransactionStableV2>,
) -> JobState {
    let mut job = job_for_statement(&work.statement(), &work.snarker);
    if let Some(transaction) = transaction {
        let (OneOrTwo::One(AvailableJobMessage::Base(base))
        | OneOrTwo::Two((AvailableJobMessage::Base(base), _))) = &mut job
        else {
            panic!("a base job")
        };
        base.transaction_with_status.data = transaction;
    }
    JobState {
        time: Timestamp::ZERO,
        id: (&job).into(),
        job,
        commitment: None,
        snark: None,
        order,
    }
}

/// Puts `job`'s base jobs in the block at `tip`, so its spec can be made.
fn in_tip_block(job: &mut JobState, tip: &v2::StateHash) {
    let (OneOrTwo::One(AvailableJobMessage::Base(first))
    | OneOrTwo::Two((AvailableJobMessage::Base(first), _))) = &mut job.job
    else {
        unreachable!()
    };
    first.state_hash.0 = tip.clone();
    if let OneOrTwo::Two((_, AvailableJobMessage::Base(second))) = &mut job.job {
        second.state_hash.0 = tip.clone();
    }
}

fn distinct_works() -> Vec<Snark> {
    let mut seen = std::collections::BTreeSet::new();
    works()
        .into_iter()
        .map(Snark::from)
        .filter(|work| seen.insert(work.job_id()))
        .collect()
}

/// P5.2, P5.2e: our worker picks jobs as OCaml's `seq` selector does: the
/// first, in scan-state order, that is not scheduled and has no stored work
/// as cheap as our fee, so it undercuts dearer work. zkApp command jobs are
/// taken in that order like any other.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L158-L173>
#[test]
fn worker_selection_follows_ocaml_seq_with_zkapp_jobs() {
    let peer_id = PeerId::from_bytes([4; 32]);
    let works = distinct_works();
    // The captured works ask 0 or 1 nanomina; price the stored one at 0.01
    // MINA (selection compares fees only, not proofs).
    let mut stored = works[0].clone();
    stored.fee.0 = 10_000_000u64.into();
    let fee = stored.fee.0.as_u64();

    let mut priced = job_state(&stored, 0, None);
    assert!(priced.is_worth_proving(fee));
    priced.snark = Some(SnarkWork {
        work: stored.clone(),
        received_t: Timestamp::ZERO,
        sender: peer_id,
    });
    // OCaml's `Fee.compare fee competing_fee < 0`: strictly cheaper only.
    assert!(!priced.is_worth_proving(fee));
    assert!(!priced.is_worth_proving(fee + 1));
    assert!(priced.is_worth_proving(fee - 1));
    // A commitment (ours, or a Rust peer's) means scheduled.
    let mut committed = job_state(&stored, 0, None);
    committed.commitment = Some(JobCommitment {
        commitment: mina_core::snark::SnarkJobCommitment::new(
            0,
            committed.id.clone(),
            stored.fee.clone(),
            stored.snarker.clone(),
        ),
        received_t: Timestamp::ZERO,
        sender: peer_id,
    });
    assert!(!committed.is_worth_proving(0));

    // A bundle with a zkApp command base job is worth proving at any fee
    // when nothing is stored, as an ordinary one.
    let zkapp = job_state(&works[1], 1, Some(zkapp_two_proofs_transaction()));
    assert!(zkapp.has_zkapp_command());
    assert!(zkapp.is_worth_proving(fee));
    assert!(zkapp.is_worth_proving(0));
    assert!(!priced.has_zkapp_command());

    let mut pool = SnarkPoolState::new();
    pool.insert(priced);
    pool.insert(zkapp);
    pool.insert(job_state(&works[2], 2, None));
    // The job with cheaper stored work (order 0) is passed over; the zkApp
    // bundle (order 1) comes first, then the ordinary one.
    let order = |jobs: Vec<&JobState>| jobs.iter().map(|job| job.order).collect::<Vec<_>>();
    assert_eq!(
        order(pool.jobs_to_prove_with_highest_priority(fee, false, 1)),
        [1]
    );
    assert_eq!(
        order(pool.jobs_to_prove_with_highest_priority(fee, false, 3)),
        [1, 2]
    );
    // At a lower fee the priced job is the first again.
    assert_eq!(
        order(pool.jobs_to_prove_with_highest_priority(fee - 1, false, 3)),
        [0, 1, 2]
    );
    // A worker whose prover has no zkApp circuits (the web node's) passes
    // the zkApp bundle over.
    assert_eq!(
        order(pool.jobs_to_prove_with_highest_priority(fee, true, 3)),
        [2]
    );
    assert_eq!(
        order(pool.jobs_to_prove_with_highest_priority(fee - 1, true, 3)),
        [0, 2]
    );
    let mut skipping = pool
        .jobs_to_prove_iter(fee - 1, true)
        .map(|job| job.order)
        .collect::<Vec<_>>();
    skipping.sort();
    assert_eq!(skipping, [0, 2]);
}

/// P5.2e's timing rule: our worker may take OCaml's 420 s reassignment wait
/// for each proof it makes in turn, as OCaml's partitioner reassigns each
/// single job, zkApp segment and merge on its own. A zkApp command of 5
/// segments is 9 proofs; bundled with another base job, 10.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_partitioner/work_partitioner.ml#L56-L86>
#[test]
fn work_timeouts_allow_the_reassignment_wait_per_proof() {
    use crate::external_snark_worker::{ExternalSnarkWorkerAction, ExternalSnarkWorkerState};

    let works = one_and_two_work();
    let (one, two) = (&works[0], &works[1]);
    let single = job_state(one, 0, None);
    assert_eq!(single.proofs_to_make(), 1);
    assert_eq!(single.work_timeout().as_secs(), 420);
    let pair = job_state(two, 0, None);
    assert_eq!(pair.proofs_to_make(), 2);
    assert_eq!(pair.work_timeout().as_secs(), 840);
    let zkapp_alone = job_state(one, 0, Some(zkapp_two_proofs_transaction()));
    assert_eq!(zkapp_alone.proofs_to_make(), 9);
    let zkapp = job_state(two, 0, Some(zkapp_two_proofs_transaction()));
    assert_eq!(zkapp.proofs_to_make(), 10);
    assert_eq!(zkapp.work_timeout().as_secs(), 4200);

    // The worker's own deadline is the job's, looked up in the pool.
    let job_id = zkapp.id.clone();
    let mut store = store_with_requested_job(zkapp.job.clone());
    edit_state(&mut store, |state| {
        state.config.snarker = Some(crate::SnarkerConfig {
            public_key: two.snarker.clone().into(),
            fee: two.fee.clone(),
            strategy: crate::SnarkerStrategy::Sequential,
            auto_commit: true,
            block_first_lead: None,
            skip_zkapp_jobs: false,
        });
        state.external_snark_worker.0.state =
            ExternalSnarkWorkerState::Working(job_id.clone(), zkapp.summary());
    });
    let at = |secs: u64| Timestamp::ZERO + std::time::Duration::from_secs(secs);
    let timeout = |secs| ExternalSnarkWorkerAction::WorkTimeout { now: at(secs) };
    assert!(!timeout(421).is_enabled(store.state(), at(421)));
    assert!(!timeout(4200).is_enabled(store.state(), at(4200)));
    assert!(timeout(4201).is_enabled(store.state(), at(4201)));
    // Without the job in the pool, the summary's: 420 s at least.
    edit_state(&mut store, |state| {
        state.snark_pool.remove(&job_id);
    });
    assert!(!timeout(420).is_enabled(store.state(), at(420)));
    assert!(timeout(421).is_enabled(store.state(), at(421)));
}

/// P5.2: the in-process worker cannot stop a proof, so a cancel waits for it.
/// A proof that finishes while the worker is `Cancelling` is still offered to
/// the pool (which keeps it while its job is wanted). P5.2e: so is one that
/// finishes after the cancel ended (a stale `WorkCancelled` let the worker
/// move on), without touching the worker's current job.
#[test]
fn late_proofs_are_always_offered() {
    use crate::external_snark_worker::{ExternalSnarkWorkerAction, ExternalSnarkWorkerState};

    let work = one_and_two_work().remove(0);
    let job_id = work.job_id();
    let other = one_and_two_work()[1].job_id();
    let snarker = |state: &mut State| {
        state.config.snarker = Some(crate::SnarkerConfig {
            public_key: work.snarker.clone().into(),
            fee: work.fee.clone(),
            strategy: crate::SnarkerStrategy::Sequential,
            auto_commit: true,
            block_first_lead: None,
            skip_zkapp_jobs: false,
        });
    };
    let at = |secs: u64| Timestamp::ZERO + std::time::Duration::from_secs(secs);
    let stored = |store: &crate::Store<Service>| {
        store
            .state()
            .snark_pool
            .get(&job_id)
            .and_then(|job| job.snark.as_ref())
            .map(|snark| snark.work.proofs.clone())
    };

    for (worker, stays) in [
        (ExternalSnarkWorkerState::Cancelling(job_id.clone()), false),
        (ExternalSnarkWorkerState::Idle, true),
        (
            ExternalSnarkWorkerState::Working(other.clone(), JobSummary::Tx(1)),
            true,
        ),
        (ExternalSnarkWorkerState::Cancelled(other.clone()), true),
    ] {
        let mut store = store_with_job(&work.to_wire().unwrap());
        edit_state(&mut store, |state| {
            snarker(state);
            state.external_snark_worker.0.state = worker.clone();
        });
        let result = ExternalSnarkWorkerAction::WorkResult {
            result: work.clone(),
        };
        assert!(result.is_enabled(store.state(), at(30)), "{worker:?}");
        store.dispatch(result);
        assert_eq!(stored(&store), Some(work.proofs.clone()), "{worker:?}");
        let after = format!("{:?}", store.state().external_snark_worker.0.state);
        if stays {
            assert_eq!(after, format!("{worker:?}"));
        } else {
            assert!(store.state().external_snark_worker.is_idle(), "{after}");
        }
    }
}

/// P5.2 gate: a 64 s proof outlived our commitment's 30 s estimate, the job
/// was chosen again while its first proof was still being admitted, and the
/// second proof was refused as not cheaper. P5.2e: our own commitments last
/// while the job is in the scan state, as OCaml's selector keeps a job it
/// handed out scheduled until it leaves the best tip's work; a Rust peer's
/// still expire at the estimate.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L142-L173>
#[test]
fn own_commitments_last_while_the_job_is_in_the_scan_state() {
    let peer_id = PeerId::from_bytes([4; 32]);
    let work = one_and_two_work().remove(1);
    let mut state = job_state(&work, 0, Some(zkapp_two_proofs_transaction()));
    let estimate = state.estimated_duration();
    let commit = |snarker: &v2::NonZeroCurvePoint| JobCommitment {
        commitment: mina_core::snark::SnarkJobCommitment::new(
            0,
            state.id.clone(),
            work.fee.clone(),
            snarker.clone(),
        ),
        received_t: Timestamp::ZERO,
        sender: peer_id,
    };
    let ours = work.snarker.clone();
    let other: v2::NonZeroCurvePoint = crate::account::AccountSecretKey::deterministic(7)
        .public_key()
        .into();
    let at = |secs: u64| Timestamp::ZERO + std::time::Duration::from_secs(secs);
    let mut pool = SnarkPoolState::new();
    let id = state.id.clone();

    state.commitment = Some(commit(&other));
    pool.insert(state.clone());
    assert!(!pool.is_commitment_timed_out(&id, at(estimate.as_secs() - 1), Some(&ours)));
    assert!(pool.is_commitment_timed_out(&id, at(estimate.as_secs()), Some(&ours)));

    state.commitment = Some(commit(&ours));
    pool.remove(&id);
    pool.insert(state);
    for secs in [estimate.as_secs(), 420, 4200, 36_000] {
        assert!(!pool.is_commitment_timed_out(&id, at(secs), Some(&ours)));
    }
    assert_eq!(
        pool.timed_out_commitments_iter(at(36_000), Some(&ours))
            .count(),
        0
    );
    // Without a worker of our own, every commitment is a peer's.
    assert!(pool.is_commitment_timed_out(&id, at(estimate.as_secs()), None));
}

/// P5.2e: a job our worker fails on (a witness that fails a circuit's
/// assertion, a prover panic, a proof failing our own check, a spec that
/// can't be made) is dropped:
/// nothing of it reaches the pool or the network, the worker takes the next
/// job, and the failed one is never chosen again while it stays in the scan
/// state.
#[test]
fn failed_jobs_are_dropped_without_retries_or_gossip() {
    use crate::external_snark_worker::{
        ExternalSnarkWorkerAction, ExternalSnarkWorkerState, ExternalSnarkWorkerWorkError,
    };

    let works = distinct_works();
    let mut state = state();
    let tip = state.transition_frontier.best_tip().unwrap().hash().clone();
    // Job 0: a zkApp command whose proof fails; job 1: one whose spec can't
    // be made (its block's protocol state is unknown), which fails while it
    // is submitted; job 2: an ordinary job. Jobs 0 and 2 are in our best
    // tip's block, so their specs can be made.
    let mut failing = job_state(&works[0], 0, Some(zkapp_two_proofs_transaction()));
    in_tip_block(&mut failing, &tip);
    let unknown = job_state(&works[1], 1, None);
    let mut next = job_state(&works[2], 2, None);
    in_tip_block(&mut next, &tip);
    let (failing_id, unknown_id, next_id) =
        (failing.id.clone(), unknown.id.clone(), next.id.clone());
    state.snark_pool.insert(failing);
    state.snark_pool.insert(unknown);
    state.snark_pool.insert(next);
    let prover = works[2].clone();
    state.config.snarker = Some(crate::SnarkerConfig {
        public_key: prover.snarker.clone().into(),
        fee: prover.fee.clone(),
        strategy: crate::SnarkerStrategy::Sequential,
        auto_commit: true,
        block_first_lead: None,
        skip_zkapp_jobs: false,
    });
    state.external_snark_worker.0.state = ExternalSnarkWorkerState::Idle;
    let mut store = redux::Store::new(
        crate::reducer,
        effects,
        Service {
            now: redux::Instant::now(),
            batches: Vec::new(),
            account: Ok(ProverAccount::Present { can_receive: true }),
            delay_accounts: false,
            account_requests: Vec::new(),
            worker_effects: Vec::new(),
            random_choices: Vec::new(),
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
    let working = |store: &crate::Store<Service>| {
        store
            .state()
            .external_snark_worker
            .working_job_id()
            .cloned()
    };

    store.dispatch(SnarkPoolAction::AutoCreateCommitment);
    assert_eq!(working(&store), Some(failing_id.clone()));

    // The zkApp witness fails its first segment's assertion (P5.1).
    store.dispatch(ExternalSnarkWorkerAction::WorkError {
        error: ExternalSnarkWorkerWorkError::Error(
            "proving failed: fee payer: not signed".to_owned(),
        ),
    });
    // Dropped: the worker moved on, past the job that failed at once, to
    // the next one.
    assert_eq!(working(&store), Some(next_id.clone()));
    let pool = &store.state().snark_pool;
    for id in [&failing_id, &unknown_id] {
        let failed = pool.get(id).unwrap();
        assert!(failed.commitment.is_some() && failed.snark.is_none());
    }
    assert_eq!(pool.admission_stats.admitted, 0);
    assert_eq!(pool.admission_stats.forwarded, 0);

    store.dispatch(ExternalSnarkWorkerAction::WorkResult {
        result: prover.clone(),
    });
    assert!(store.state().external_snark_worker.is_idle());
    assert_eq!(store.state().snark_pool.admission_stats.forwarded, 1);

    // Hours later the failed job is still ours and not chosen again.
    for hours in [1, 10] {
        store.service.now += std::time::Duration::from_secs(3600 * hours);
        // Enabling conditions see the last action's time: move it on first.
        store.dispatch(SnarkPoolAction::AutoCreateCommitment);
        assert!(store.dispatch(SnarkPoolAction::CheckTimeouts));
        store.dispatch(SnarkPoolAction::AutoCreateCommitment);
        assert!(store.state().external_snark_worker.is_idle());
        for id in [&failing_id, &unknown_id] {
            let failed = store.state().snark_pool.get(id).unwrap();
            assert!(failed.commitment.is_some() && failed.snark.is_none());
        }
    }
    let pool = &store.state().snark_pool;
    assert_eq!(pool.admission_stats.admitted, 1);
    assert_eq!(pool.admission_stats.forwarded, 1);
    assert!(pool.get(&next_id).unwrap().snark.is_some());
}

/// Our worker's fee and the stored work's on public devnet in P5.2e: 0.0043
/// MINA and 0.01 MINA.
const OUR_FEE: u64 = 4_300_000;
const DEARER_FEE: u64 = 10_000_000;

fn our_snarker(public_key: crate::account::AccountPublicKey) -> crate::SnarkerConfig {
    crate::SnarkerConfig {
        public_key,
        fee: v2::CurrencyFeeStableV1(OUR_FEE.into()),
        strategy: crate::SnarkerStrategy::Sequential,
        auto_commit: true,
        block_first_lead: None,
        skip_zkapp_jobs: false,
    }
}

/// P5.2f: on public devnet our worker sat idle while zkApp bundles held
/// dearer work. The selector chose them, but the commitment gate refused
/// any job with stored work. The gate is now the selector's rule at our
/// configured fee: dearer stored work can be undercut; work as cheap as
/// ours, or any commitment, cannot. zkApp command bundles are no different.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L158-L173>
#[test]
fn commitments_undercut_dearer_stored_work() {
    let peer_id = PeerId::from_bytes([4; 32]);
    let work = distinct_works().remove(0);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let with_stored = |mut job: JobState, fee: u64| {
        let mut stored = work.clone();
        stored.fee.0 = fee.into();
        job.snark = Some(SnarkWork {
            work: stored,
            received_t: Timestamp::ZERO,
            sender: peer_id,
        });
        job
    };
    let with_commitment = |mut job: JobState| {
        job.commitment = Some(JobCommitment {
            commitment: mina_core::snark::SnarkJobCommitment::new(
                0,
                job.id.clone(),
                work.fee.clone(),
                work.snarker.clone(),
            ),
            received_t: Timestamp::ZERO,
            sender: peer_id,
        });
        job
    };
    let gate = |job: JobState, snarker: bool| {
        let mut state = state();
        state.config.snarker = snarker.then(|| our_snarker(ours.clone()));
        let job_id = job.id.clone();
        state.snark_pool.insert(job);
        let enabled = SnarkPoolAction::CommitmentCreate {
            job_id: job_id.clone(),
        }
        .is_enabled(&state, Timestamp::ZERO);
        assert_eq!(
            enabled,
            snarker && state.snark_pool.should_create_commitment(&job_id, OUR_FEE)
        );
        enabled
    };

    for zkapp in [None, Some(zkapp_two_proofs_transaction())] {
        let job = || job_state(&work, 0, zkapp.clone());
        assert_eq!(job().has_zkapp_command(), zkapp.is_some());
        // Nothing stored: open, as before.
        assert!(gate(job(), true));
        // Dearer stored work (P5.2e's 54 zkApp bundles): open to undercut.
        assert!(gate(with_stored(job(), DEARER_FEE), true));
        assert!(gate(with_stored(job(), OUR_FEE + 1), true));
        // Work as cheap as ours, or cheaper: ours would not be taken.
        for fee in [OUR_FEE, OUR_FEE - 1, 0] {
            assert!(!gate(with_stored(job(), fee), true), "{fee}");
        }
        // A commitment means scheduled, with or without stored work.
        assert!(!gate(with_commitment(job()), true));
        assert!(!gate(with_commitment(with_stored(job(), DEARER_FEE)), true));
        // Without a worker of ours, nothing is committed.
        assert!(!gate(with_stored(job(), DEARER_FEE), false));
    }
}

/// P5.2f, through the reducers: our worker commits to the oldest job whose
/// stored work is dearer than our fee, proves it, and its work replaces the
/// dearer work, as `WorkAdd` takes strictly cheaper work. Producers then
/// take ours, and it is forwarded on libp2p and offered again on the pull
/// protocol, as work for a job that had none. A zkApp bundle goes first,
/// then an ordinary job; a job whose stored work is as cheap as ours is left
/// alone.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L185-L208>
#[test]
fn our_worker_undercuts_dearer_stored_work() {
    use crate::external_snark_worker::{ExternalSnarkWorkerAction, ExternalSnarkWorkerState};

    let peer_id = PeerId::from_bytes([4; 32]);
    let works = distinct_works();
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let our_key: v2::NonZeroCurvePoint = ours.clone().into();
    let theirs: v2::NonZeroCurvePoint = crate::account::AccountSecretKey::deterministic(9)
        .public_key()
        .into();
    let mut state = state();
    let tip = state.transition_frontier.best_tip().unwrap().hash().clone();
    // Jobs 0 (a zkApp command) and 1 hold another prover's dearer work, job
    // 2 its work at our fee. All are in our best tip's block, so their specs
    // can be made.
    let mut jobs = Vec::new();
    for (order, (fee, zkapp)) in [(DEARER_FEE, true), (DEARER_FEE, false), (OUR_FEE, false)]
        .into_iter()
        .enumerate()
    {
        let mut job = job_state(
            &works[order],
            order,
            zkapp.then(zkapp_two_proofs_transaction),
        );
        in_tip_block(&mut job, &tip);
        let mut stored = works[order].clone();
        stored.snarker = theirs.clone();
        stored.fee.0 = fee.into();
        job.snark = Some(SnarkWork {
            work: stored,
            received_t: Timestamp::ZERO,
            sender: peer_id,
        });
        jobs.push(job.id.clone());
        state.snark_pool.insert(job);
    }
    // The other prover's work was admitted at this tip.
    state.snark_pool.prover_accounts.insert(
        theirs.clone().into(),
        (tip, ProverAccount::Present { can_receive: true }),
    );
    state.config.snarker = Some(our_snarker(ours));
    state.external_snark_worker.0.state = ExternalSnarkWorkerState::Idle;
    let mut store = redux::Store::new(
        crate::reducer,
        effects,
        Service {
            now: redux::Instant::now(),
            batches: Vec::new(),
            account: Ok(ProverAccount::Present { can_receive: true }),
            delay_accounts: false,
            account_requests: Vec::new(),
            worker_effects: Vec::new(),
            random_choices: Vec::new(),
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
    let working = |store: &crate::Store<Service>| {
        store
            .state()
            .external_snark_worker
            .working_job_id()
            .cloned()
    };
    let stored = |store: &crate::Store<Service>, id: &mina_core::snark::SnarkJobId| {
        let job = store.state().snark_pool.get(id).unwrap();
        let work = &job.snark.as_ref().unwrap().work;
        (work.snarker.clone(), work.fee.0.as_u64())
    };
    assert_eq!(store.state().snark_pool.completed_snarks_iter().count(), 3);
    let pulled = store.state().snark_pool.last_index();

    // The zkApp bundle with dearer work is the oldest job: ours to prove.
    store.dispatch(SnarkPoolAction::AutoCreateCommitment);
    assert_eq!(working(&store), Some(jobs[0].clone()));
    let job = store.state().snark_pool.get(&jobs[0]).unwrap();
    let commitment = &job.commitment.as_ref().unwrap().commitment;
    assert_eq!(commitment.snarker, our_key);
    assert_eq!(commitment.fee.0.as_u64(), OUR_FEE);

    // Each proof replaces the dearer work; the worker then takes the next
    // job with dearer work, and none after it.
    for (index, next) in [(0, Some(jobs[1].clone())), (1, None)] {
        store.dispatch(ExternalSnarkWorkerAction::WorkResult {
            result: Snark {
                snarker: our_key.clone(),
                fee: v2::CurrencyFeeStableV1(OUR_FEE.into()),
                proofs: works[index].proofs.clone(),
            },
        });
        assert_eq!(stored(&store, &jobs[index]), (our_key.clone(), OUR_FEE));
        assert_eq!(working(&store), next);
    }
    assert!(store.state().external_snark_worker.is_idle());
    store.dispatch(SnarkPoolAction::AutoCreateCommitment);
    assert!(store.state().external_snark_worker.is_idle());

    let pool = &store.state().snark_pool;
    assert_eq!(pool.admission_stats.admitted, 2);
    assert_eq!(pool.admission_stats.forwarded, 2);
    assert_eq!(pool.admission_stats.rejected_after_verification, 0);
    // Work as cheap as ours keeps its job.
    assert_eq!(stored(&store, &jobs[2]), (theirs.clone(), OUR_FEE));
    assert!(pool.get(&jobs[2]).unwrap().commitment.is_none());
    // Producers take our work for jobs 0 and 1.
    let mut completed = pool
        .completed_snarks_iter()
        .map(|work| (work.job_id(), work.snarker.clone(), work.fee.0.as_u64()))
        .collect::<Vec<_>>();
    completed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![
        (jobs[0].clone(), our_key.clone(), OUR_FEE),
        (jobs[1].clone(), our_key.clone(), OUR_FEE),
        (jobs[2].clone(), theirs, OUR_FEE),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(completed, expected);
    // A Rust peer that had pulled the whole pool is offered ours again.
    let (snarks, _, _) = pool.next_snarks_to_send((pulled + 1, u8::MAX));
    let snarks = snarks
        .into_iter()
        .map(|info| (info.job_id, info.prover, info.fee.0.as_u64()))
        .collect::<Vec<_>>();
    assert_eq!(
        snarks,
        [
            (jobs[0].clone(), our_key.clone(), OUR_FEE),
            (jobs[1].clone(), our_key, OUR_FEE),
        ]
    );
}

// P5.4: the operator starts, re-prices and stops the worker while the node
// runs. The worker's service is simulated: the tests answer with its events.

fn control_store(
    allowed: bool,
) -> (
    crate::Store<Service>,
    Vec<mina_core::snark::SnarkJobId>,
    Vec<Snark>,
) {
    let works = distinct_works();
    let mut state = state();
    state.config.worker_control = allowed;
    let tip = state.transition_frontier.best_tip().unwrap().hash().clone();
    // Three ordinary jobs in our best tip's block, so their specs can be made.
    let mut ids = Vec::new();
    for (order, work) in works.iter().take(3).enumerate() {
        let mut job = job_state(work, order, None);
        in_tip_block(&mut job, &tip);
        ids.push(job.id.clone());
        state.snark_pool.insert(job);
    }
    let store = redux::Store::new(
        crate::reducer,
        effects,
        Service {
            now: redux::Instant::now(),
            batches: Vec::new(),
            account: Ok(ProverAccount::Present { can_receive: true }),
            delay_accounts: false,
            account_requests: Vec::new(),
            worker_effects: Vec::new(),
            random_choices: Vec::new(),
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
    (store, ids, works)
}

fn setting(
    public_key: &crate::account::AccountPublicKey,
    fee: u64,
) -> Option<crate::SnarkerSetting> {
    Some(crate::SnarkerSetting {
        public_key: public_key.clone(),
        fee,
    })
}

fn our_work(key: &v2::NonZeroCurvePoint, fee: u64, work: &Snark) -> Snark {
    Snark {
        snarker: key.clone(),
        fee: v2::CurrencyFeeStableV1(fee.into()),
        proofs: work.proofs.clone(),
    }
}

fn take_worker_effects(store: &mut crate::Store<Service>) -> Vec<String> {
    std::mem::take(&mut store.service.worker_effects)
}

fn commitment_of(
    store: &crate::Store<Service>,
    id: &mina_core::snark::SnarkJobId,
) -> Option<(v2::NonZeroCurvePoint, u64)> {
    let job = store.state().snark_pool.get(id)?;
    let commitment = &job.commitment.as_ref()?.commitment;
    Some((commitment.snarker.clone(), commitment.fee.0.as_u64()))
}

fn stored_work(
    store: &crate::Store<Service>,
    id: &mina_core::snark::SnarkJobId,
) -> Option<(v2::NonZeroCurvePoint, u64)> {
    let work = &store.state().snark_pool.get(id)?.snark.as_ref()?.work;
    Some((work.snarker.clone(), work.fee.0.as_u64()))
}

/// Control changes where fees go: refused unless the node was started with
/// `--allow-worker-control` (`GlobalConfig::worker_control`), through the
/// RPC and the action alike.
#[test]
fn worker_control_reports_the_job_strategy() {
    use crate::SnarkerStrategy::{Random, Sequential};

    let (mut store, _, _) = control_store(true);
    // Off: the strategy a worker started from the dashboard would use.
    for strategy in [Random, Sequential] {
        edit_state(&mut store, |state| state.config.snarker_strategy = strategy);
        assert_eq!(store.state().snark_worker_control().strategy, strategy);
    }
    // On: the running worker's, which it took from the same setting.
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    store.dispatch(crate::rpc::RpcAction::SnarkWorkerControlSet {
        rpc_id: crate::rpc::RpcId::new_unchecked(1, 1),
        setting: setting(&ours, OUR_FEE),
    });
    let control = store.state().snark_worker_control();
    assert!(control.enabled);
    assert_eq!(control.strategy, Sequential);
    // The dashboard reads the variant's name.
    assert_eq!(
        serde_json::to_value(&control).unwrap()["strategy"],
        "Sequential"
    );
}

#[test]
fn worker_control_is_refused_unless_allowed() {
    use crate::external_snark_worker::ExternalSnarkWorkerAction;

    let (mut store, _, _) = control_store(false);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let on = setting(&ours, OUR_FEE);
    let configure = ExternalSnarkWorkerAction::Configure {
        setting: on.clone(),
    };
    assert!(!configure.is_enabled(store.state(), Timestamp::ZERO));
    assert!(!store.dispatch(configure));
    store.dispatch(crate::rpc::RpcAction::SnarkWorkerControlSet {
        rpc_id: crate::rpc::RpcId::new_unchecked(1, 1),
        setting: on.clone(),
    });
    assert!(store.state().config.snarker.is_none());
    assert!(take_worker_effects(&mut store).is_empty());
    let control = store.state().snark_worker_control();
    assert!(!control.allowed && !control.enabled);

    // Allowed, the same request starts it.
    edit_state(&mut store, |state| state.config.worker_control = true);
    store.dispatch(crate::rpc::RpcAction::SnarkWorkerControlSet {
        rpc_id: crate::rpc::RpcId::new_unchecked(1, 2),
        setting: on.clone(),
    });
    assert_eq!(
        store.state().config.snarker.as_ref().map(|c| c.setting()),
        on
    );
    assert_eq!(take_worker_effects(&mut store), [format!("save {OUR_FEE}")]);
    let control = store.state().snark_worker_control();
    assert!(control.allowed && control.enabled);
    assert_eq!(control.fee, Some(OUR_FEE));
    // Its key's work is looked for in blocks from now on.
    assert_eq!(
        store.state().external_snark_worker.included_work().keys,
        [v2::NonZeroCurvePoint::from(ours)]
    );
}

/// A node started without a worker: the dashboard starts it, re-prices it
/// while it proves, and stops it while it proves. Work proved under the
/// setting is offered; the proof in flight at a re-price or a stop is
/// dropped, and its job released, so the worker takes it again at its new
/// fee (P5.4's review: a kept commitment, which never times out, stranded
/// it).
#[test]
fn the_worker_starts_reprices_and_stops_at_runtime() {
    use crate::external_snark_worker::{ExternalSnarkWorkerAction, ExternalSnarkWorkerState};

    const NEW_FEE: u64 = 3_000_000;
    let (mut store, ids, works) = control_store(true);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let our_key: v2::NonZeroCurvePoint = ours.clone().into();
    let working = |store: &crate::Store<Service>| {
        store
            .state()
            .external_snark_worker
            .working_job_id()
            .cloned()
    };

    // No worker at startup.
    assert!(!store.dispatch(ExternalSnarkWorkerAction::Start));

    // Started from the dashboard: `CheckTimeouts` starts the worker, whose
    // service answers `Started`; it takes the oldest job and proves it.
    assert!(store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&ours, OUR_FEE),
    }));
    assert!(store.dispatch(ExternalSnarkWorkerAction::Start));
    assert!(store.dispatch(ExternalSnarkWorkerAction::Started));
    assert_eq!(
        take_worker_effects(&mut store),
        [
            format!("save {OUR_FEE}"),
            format!("start {OUR_FEE}"),
            "submit".to_owned()
        ]
    );
    assert_eq!(working(&store), Some(ids[0].clone()));
    assert_eq!(
        commitment_of(&store, &ids[0]),
        Some((our_key.clone(), OUR_FEE))
    );
    store.dispatch(ExternalSnarkWorkerAction::WorkResult {
        result: our_work(&our_key, OUR_FEE, &works[0]),
    });
    assert_eq!(
        stored_work(&store, &ids[0]),
        Some((our_key.clone(), OUR_FEE))
    );
    assert_eq!(working(&store), Some(ids[1].clone()));
    take_worker_effects(&mut store);

    // Re-priced while it proves job 1: the worker is stopped, job 1 is
    // released, and the worker takes no job until it starts again.
    assert!(store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&ours, NEW_FEE),
    }));
    assert_eq!(
        take_worker_effects(&mut store),
        ["kill".to_owned(), format!("save {NEW_FEE}")]
    );
    assert!(matches!(
        store.state().external_snark_worker.0.state,
        ExternalSnarkWorkerState::Killing
    ));
    assert_eq!(commitment_of(&store, &ids[1]), None);
    // Job 0, proved, keeps its work and our commitment.
    assert_eq!(
        commitment_of(&store, &ids[0]),
        Some((our_key.clone(), OUR_FEE))
    );
    // Job 1's proof, made for the old fee, finishes before the worker
    // stops: not offered.
    let admitted = store.state().snark_pool.admission_stats.admitted;
    store.dispatch(ExternalSnarkWorkerAction::WorkResult {
        result: our_work(&our_key, OUR_FEE, &works[1]),
    });
    assert_eq!(stored_work(&store, &ids[1]), None);
    assert_eq!(store.state().snark_pool.admission_stats.admitted, admitted);
    assert_eq!(working(&store), None);
    assert!(!store.dispatch(ExternalSnarkWorkerAction::Start));
    store.dispatch(ExternalSnarkWorkerAction::Killed);
    assert!(store.dispatch(ExternalSnarkWorkerAction::Start));
    assert!(store.dispatch(ExternalSnarkWorkerAction::Started));
    assert_eq!(
        take_worker_effects(&mut store),
        [format!("start {NEW_FEE}"), "submit".to_owned()]
    );
    // Job 1 is taken again, at the new fee.
    assert_eq!(working(&store), Some(ids[1].clone()));
    assert_eq!(
        commitment_of(&store, &ids[1]),
        Some((our_key.clone(), NEW_FEE))
    );

    // Stopped while it proves job 1: our commitments are released and the
    // proof in flight is dropped, not offered.
    assert!(store.dispatch(ExternalSnarkWorkerAction::Configure { setting: None }));
    assert_eq!(
        take_worker_effects(&mut store),
        ["kill".to_owned(), "save off".to_owned()]
    );
    assert!(store.state().config.snarker.is_none());
    for id in &ids[..2] {
        assert_eq!(commitment_of(&store, id), None);
    }
    store.dispatch(ExternalSnarkWorkerAction::WorkResult {
        result: our_work(&our_key, NEW_FEE, &works[1]),
    });
    assert_eq!(stored_work(&store, &ids[1]), None);
    assert_eq!(store.state().snark_pool.admission_stats.admitted, admitted);
    store.dispatch(ExternalSnarkWorkerAction::Killed);
    assert!(!store.dispatch(ExternalSnarkWorkerAction::Start));
    // The work proved before the changes stays in the pool for producers.
    assert_eq!(
        stored_work(&store, &ids[0]),
        Some((our_key.clone(), OUR_FEE))
    );

    // The dashboard sees the outcomes, newest first, with their times; O2's
    // counter counts every proof made.
    let control = store.state().snark_worker_control();
    assert!(!control.enabled);
    let recent = control
        .recent
        .iter()
        .map(|o| (o.job_id.clone(), o.fee, o.offered, o.duration_ms.is_some()))
        .collect::<Vec<_>>();
    assert_eq!(
        recent,
        [
            (ids[1].clone(), Some(NEW_FEE), false, true),
            (ids[1].clone(), Some(OUR_FEE), false, true),
            (ids[0].clone(), Some(OUR_FEE), true, true),
        ]
    );
    assert_eq!(store.state().external_snark_worker.0.proofs_made, 3);
    // F-dash2: the dashboard reads the same counter from the control.
    assert_eq!(control.proofs_made, 3);
}

/// P5.4's review: our proof can wait for its prover's account while the
/// operator stops or re-prices the worker. It is then not admitted, nor
/// broadcast, and its job is released; with no change it is admitted.
#[test]
fn own_work_waiting_for_admission_is_dropped_after_a_stop_or_reprice() {
    use crate::external_snark_worker::ExternalSnarkWorkerAction;

    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let our_key: v2::NonZeroCurvePoint = ours.clone().into();
    for change in [None, Some(None), Some(setting(&ours, OUR_FEE - 1))] {
        let (mut store, ids, works) = control_store(true);
        store.dispatch(ExternalSnarkWorkerAction::Configure {
            setting: setting(&ours, OUR_FEE),
        });
        store.dispatch(ExternalSnarkWorkerAction::Start);
        store.dispatch(ExternalSnarkWorkerAction::Started);
        // The proof of job 0 waits for the ledger read of its prover.
        store.service.delay_accounts = true;
        store.dispatch(ExternalSnarkWorkerAction::WorkResult {
            result: our_work(&our_key, OUR_FEE, &works[0]),
        });
        let id = *store.service.account_requests.last().unwrap();
        if let Some(setting) = change.clone() {
            assert!(store.dispatch(ExternalSnarkWorkerAction::Configure { setting }));
        }
        store.dispatch(crate::ledger::read::LedgerReadAction::Success {
            id,
            response: crate::ledger::read::LedgerReadResponse::SnarkPoolProver(Ok(
                ProverAccount::Present { can_receive: true },
            )),
        });
        let pool = &store.state().snark_pool;
        if change.is_none() {
            assert_eq!(
                stored_work(&store, &ids[0]),
                Some((our_key.clone(), OUR_FEE))
            );
            assert_eq!(pool.admission_stats.forwarded, 1);
        } else {
            assert_eq!(stored_work(&store, &ids[0]), None, "{change:?}");
            assert_eq!(pool.admission_stats.forwarded, 0, "{change:?}");
            assert_eq!(pool.admission_stats.admitted, 0, "{change:?}");
            // Open again: no commitment of ours keeps it.
            assert_eq!(commitment_of(&store, &ids[0]), None, "{change:?}");
        }
        // Local work is admitted only for the current setting, also when
        // `WorkAdd` comes another way.
        let add = SnarkPoolAction::WorkAdd {
            snark: our_work(&our_key, OUR_FEE, &works[2]),
            sender: store.state().p2p.my_id(),
            is_sender_local: true,
        };
        if change.is_some() {
            assert!(
                !add.is_enabled(store.state(), Timestamp::ZERO),
                "{change:?}"
            );
        }
    }
}

/// A new key: the old key's commitments are released, so the jobs are open
/// again, and a proof made for the old key is dropped.
#[test]
fn a_new_key_releases_the_old_keys_jobs() {
    use crate::external_snark_worker::ExternalSnarkWorkerAction;

    let (mut store, ids, works) = control_store(true);
    let old = crate::account::AccountSecretKey::deterministic(8).public_key();
    let new = crate::account::AccountSecretKey::deterministic(9).public_key();
    store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&old, OUR_FEE),
    });
    store.dispatch(ExternalSnarkWorkerAction::Start);
    store.dispatch(ExternalSnarkWorkerAction::Started);
    assert_eq!(
        commitment_of(&store, &ids[0]),
        Some((old.clone().into(), OUR_FEE))
    );

    store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&new, OUR_FEE),
    });
    assert_eq!(commitment_of(&store, &ids[0]), None);
    store.dispatch(ExternalSnarkWorkerAction::WorkResult {
        result: our_work(&old.clone().into(), OUR_FEE, &works[0]),
    });
    assert_eq!(stored_work(&store, &ids[0]), None);
    store.dispatch(ExternalSnarkWorkerAction::Killed);
    store.dispatch(ExternalSnarkWorkerAction::Start);
    store.dispatch(ExternalSnarkWorkerAction::Started);
    // Job 0 is open again: the new key takes it.
    assert_eq!(
        commitment_of(&store, &ids[0]),
        Some((new.clone().into(), OUR_FEE))
    );
    // Both keys' work is looked for in blocks.
    assert_eq!(
        store.state().external_snark_worker.included_work().keys,
        [v2::NonZeroCurvePoint::from(old), new.into()]
    );
}

/// A worker that stopped on an error starts again with a new setting; one
/// already stopped, or stopping, is left to start or stop by itself; the same
/// setting again doesn't restart a running worker.
#[test]
fn configure_restarts_only_what_needs_it() {
    use crate::external_snark_worker::{
        ExternalSnarkWorkerAction, ExternalSnarkWorkerError, ExternalSnarkWorkerState,
    };

    let (mut store, _, _) = control_store(true);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    edit_state(&mut store, |state| {
        state.external_snark_worker.0.state =
            ExternalSnarkWorkerState::Error(ExternalSnarkWorkerError::NotRunning, false);
    });
    assert!(!store.dispatch(ExternalSnarkWorkerAction::Start));
    store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&ours, OUR_FEE),
    });
    assert!(store.dispatch(ExternalSnarkWorkerAction::Start));
    assert!(store.dispatch(ExternalSnarkWorkerAction::Started));
    take_worker_effects(&mut store);

    // The same setting: nothing to restart.
    store.dispatch(ExternalSnarkWorkerAction::Configure {
        setting: setting(&ours, OUR_FEE),
    });
    assert_eq!(take_worker_effects(&mut store), [format!("save {OUR_FEE}")]);

    // Stopping twice kills once.
    store.dispatch(ExternalSnarkWorkerAction::Configure { setting: None });
    store.dispatch(ExternalSnarkWorkerAction::Configure { setting: None });
    assert_eq!(
        take_worker_effects(&mut store),
        [
            "kill".to_owned(),
            "save off".to_owned(),
            "save off".to_owned()
        ]
    );
}

/// When a block joins the best chain, the log records the bundles it bought
/// from our worker's key (the reducer's hook on `Synced`).
#[test]
fn a_new_best_tip_with_our_work_is_logged() {
    use crate::transition_frontier::sync::{
        TransitionFrontierRootSnarkedLedgerUpdates, TransitionFrontierSyncState,
    };

    let (mut store, _, works) = control_store(true);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let our_key: v2::NonZeroCurvePoint = ours.clone().into();
    store.dispatch(
        crate::external_snark_worker::ExternalSnarkWorkerAction::Configure {
            setting: setting(&ours, OUR_FEE),
        },
    );
    let root = store
        .state()
        .transition_frontier
        .best_chain
        .last()
        .unwrap()
        .clone();
    let mut block = (*root.block.block).clone();
    block.header.protocol_state.previous_state_hash = root.hash().clone();
    block
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = (root.height() + 1).into();
    let diff = &mut block.body.staged_ledger_diff.diff;
    diff.1 = None;
    diff.0.completed_works = works
        .iter()
        .take(2)
        .map(|work| our_work(&our_key, OUR_FEE, work).to_wire().unwrap())
        .collect();
    let tip = mina_core::block::AppliedBlock {
        block: mina_core::block::BlockWithHash::try_new(std::sync::Arc::new(block)).unwrap(),
        just_emitted_a_proof: false,
    };
    edit_state(&mut store, |state| {
        state.transition_frontier.sync = TransitionFrontierSyncState::CommitSuccess {
            time: Timestamp::ZERO,
            chain: vec![root.clone(), tip.clone()],
            root_snarked_ledger_updates: TransitionFrontierRootSnarkedLedgerUpdates::default(),
            needed_protocol_states: Default::default(),
        };
    });
    assert!(store.dispatch(crate::TransitionFrontierAction::Synced {
        needed_protocol_states: Default::default(),
    }));
    let log = store.state().external_snark_worker.included_work();
    let entry = log.entries.back().unwrap();
    assert_eq!(&entry.state_hash, tip.hash());
    assert_eq!(entry.bundles, 2);
    assert_eq!(entry.fees, 2 * OUR_FEE);
    assert_eq!(
        entry.status,
        crate::external_snark_worker::IncludedWorkStatus::Pending
    );
}

// SW-rand: the worker picks jobs at random by default, as OCaml's does
// (`--work-selection rand`); `seq` stays.

/// Both selections take only the jobs OCaml's selector would: not scheduled
/// (no commitment), no stored work as cheap as our fee, and no zkApp command
/// for a prover without zkApp circuits. `seq` takes the first in scan-state
/// order; `rand` chooses among all of them, and takes the one chosen.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L158-L173>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/random.ml#L4-L11>
#[test]
fn worker_selection_takes_only_eligible_jobs_seq_or_rand() {
    use crate::{
        external_snark_worker::ExternalSnarkWorkerState,
        SnarkerStrategy::{self, Random, Sequential},
    };

    let peer_id = PeerId::from_bytes([4; 32]);
    let works = distinct_works();
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let our_key: v2::NonZeroCurvePoint = ours.clone().into();
    let theirs: v2::NonZeroCurvePoint = crate::account::AccountSecretKey::deterministic(9)
        .public_key()
        .into();
    let mut pool_state = state();
    let tip = pool_state
        .transition_frontier
        .best_tip()
        .unwrap()
        .hash()
        .clone();
    // Orders 0 to 2 can't be taken: a peer's commitment, stored work at our
    // fee, stored work below it. Orders 3 (dearer stored work), 4 and 5 (a
    // zkApp command) can.
    let mut ids = Vec::new();
    for (order, work) in works.iter().take(6).enumerate() {
        let mut job = job_state(work, order, (order == 5).then(zkapp_two_proofs_transaction));
        in_tip_block(&mut job, &tip);
        let stored_fee = match order {
            1 => Some(OUR_FEE),
            2 => Some(OUR_FEE - 1),
            3 => Some(DEARER_FEE),
            _ => None,
        };
        job.snark = stored_fee.map(|fee| SnarkWork {
            work: Snark {
                snarker: theirs.clone(),
                fee: v2::CurrencyFeeStableV1(fee.into()),
                proofs: work.proofs.clone(),
            },
            received_t: Timestamp::ZERO,
            sender: peer_id,
        });
        if order == 0 {
            job.commitment = Some(JobCommitment {
                commitment: mina_core::snark::SnarkJobCommitment::new(
                    0,
                    job.id.clone(),
                    v2::CurrencyFeeStableV1(DEARER_FEE.into()),
                    theirs.clone(),
                ),
                received_t: Timestamp::ZERO,
                sender: peer_id,
            });
        }
        ids.push(job.id.clone());
        pool_state.snark_pool.insert(job);
    }
    assert!(pool_state
        .snark_pool
        .get(&ids[5])
        .unwrap()
        .has_zkapp_command());

    let take = |strategy: SnarkerStrategy, skip_zkapp_jobs: bool| {
        let mut state = pool_state.clone();
        state.config.snarker = Some(crate::SnarkerConfig {
            strategy,
            skip_zkapp_jobs,
            ..our_snarker(ours.clone())
        });
        state.external_snark_worker.0.state = ExternalSnarkWorkerState::Idle;
        let mut store = redux::Store::new(
            crate::reducer,
            effects,
            Service {
                now: redux::Instant::now(),
                batches: Vec::new(),
                account: Ok(ProverAccount::Present { can_receive: true }),
                delay_accounts: false,
                account_requests: Vec::new(),
                worker_effects: Vec::new(),
                random_choices: Vec::new(),
            },
            redux::SystemTime::UNIX_EPOCH,
            state,
        );
        store.dispatch(SnarkPoolAction::AutoCreateCommitment);
        let working = store
            .state()
            .external_snark_worker
            .working_job_id()
            .cloned();
        assert_eq!(
            commitment_of(&store, working.as_ref().unwrap()),
            Some((our_key.clone(), OUR_FEE))
        );
        let choices = store
            .service
            .random_choices
            .iter()
            .map(|(choices, count)| {
                let mut orders = choices
                    .iter()
                    .map(|id| ids.iter().position(|i| i == id).unwrap())
                    .collect::<Vec<_>>();
                orders.sort();
                (orders, *count)
            })
            .collect::<Vec<_>>();
        let working = ids.iter().position(|id| Some(id) == working.as_ref());
        (working, choices)
    };

    // `seq`: the first job it may take, with or without zkApp jobs.
    assert_eq!(take(Sequential, false), (Some(3), vec![]));
    assert_eq!(take(Sequential, true), (Some(3), vec![]));
    // `rand`: one job, chosen among exactly the jobs `seq` may take; the
    // test's service chooses the last, which is the job taken.
    assert_eq!(take(Random, false), (Some(5), vec![(vec![3, 4, 5], 1)]));
    // A prover without zkApp circuits (the web node's) has no zkApp job
    // among its choices.
    assert_eq!(take(Random, true), (Some(4), vec![(vec![3, 4], 1)]));
}

/// A worker the operator starts while the node runs (P5.4) has the node's
/// job selection, `--snarker-strategy`, as one started with the node, also
/// after a stop; a re-priced worker keeps its own. Before SW-rand it was
/// always `seq`.
#[test]
fn a_worker_started_at_runtime_has_the_nodes_strategy() {
    use crate::{
        external_snark_worker::ExternalSnarkWorkerAction,
        SnarkerConfig,
        SnarkerStrategy::{self, Random, Sequential},
    };

    assert_eq!(SnarkerStrategy::default(), Random);
    let ours = crate::account::AccountSecretKey::deterministic(8).public_key();
    let on = setting(&ours, OUR_FEE).unwrap();
    for (strategy, other) in [(Random, Sequential), (Sequential, Random)] {
        // What `--run-snarker` gives, with the node's strategy.
        let config = SnarkerConfig::with_setting(&on, None, strategy);
        assert_eq!(config.setting(), on);
        assert_eq!(config.strategy, strategy);
        assert!(config.auto_commit && !config.skip_zkapp_jobs);
        assert_eq!(config.block_first_lead, None);
        let repriced = setting(&ours, OUR_FEE + 1).unwrap();
        let config = SnarkerConfig::with_setting(&repriced, Some(&config), other);
        assert_eq!((config.setting(), config.strategy), (repriced, strategy));
    }

    for strategy in [Random, Sequential] {
        let (mut store, ids, _) = control_store(true);
        edit_state(&mut store, |state| {
            state.config.snarker_strategy = strategy;
            state.config.block_first_lead = Some(std::time::Duration::from_secs(420));
        });
        let started = |store: &mut crate::Store<Service>| {
            assert!(store.dispatch(ExternalSnarkWorkerAction::Configure {
                setting: setting(&ours, OUR_FEE),
            }));
            let config = store.state().config.snarker.as_ref().unwrap();
            assert_eq!(config.strategy, strategy);
            assert_eq!(
                config.block_first_lead,
                Some(std::time::Duration::from_secs(420))
            );
            assert!(store.dispatch(ExternalSnarkWorkerAction::Start));
            assert!(store.dispatch(ExternalSnarkWorkerAction::Started));
            let working = store.state().external_snark_worker.working_job_id();
            ids.iter().position(|id| Some(id) == working)
        };
        // `seq` takes the oldest of the three jobs; `rand` chooses among all
        // three, and the test's service chooses the last.
        let first = started(&mut store);
        let choices = std::mem::take(&mut store.service.random_choices);
        match strategy {
            Sequential => {
                assert_eq!(first, Some(0));
                assert!(choices.is_empty());
            }
            Random => {
                assert_eq!(first, Some(2));
                assert_eq!(choices.len(), 1);
                let (mut chosen_among, count) = choices[0].clone();
                assert_eq!(count, 1);
                chosen_among.sort();
                let mut all = ids.clone();
                all.sort();
                assert_eq!(chosen_among, all);
            }
        }
        // Stopped, then started again: still the node's strategy.
        assert!(store.dispatch(ExternalSnarkWorkerAction::Configure { setting: None }));
        store.dispatch(ExternalSnarkWorkerAction::Killed);
        assert!(started(&mut store).is_some());
    }
}

/// RC7-FIXES: `--run-snarker KEY --snarker-fee 0.004` for a key with no
/// account starts, as the ledger isn't there yet to check it; once the pool
/// refuses the worker's first proof by OCaml's prover rule against the
/// ledger, the node warns once for that key and fee.
#[test]
fn rc7_a_refused_worker_fee_is_warned_once_per_key_and_fee() {
    let mut pool = SnarkPoolState::new();
    let work = Snark::from(works().remove(0));
    let key = work.snarker.clone();
    let low = our_work(&key, 4_000_000, &work);
    assert!(pool.warn_own_fee_refused(&low, ProverAccount::Missing, Timestamp::ZERO));
    assert!(!pool.warn_own_fee_refused(&low, ProverAccount::Missing, Timestamp::ZERO));
    // A new fee that is still refused: warned again.
    let other = our_work(&key, 5_000_000, &work);
    assert!(pool.warn_own_fee_refused(&other, ProverAccount::Missing, Timestamp::ZERO));
    // No refusal, no warning: 1 MINA creates the account, or it exists.
    let creating = our_work(&key, 1_000_000_000, &work);
    assert!(!pool.warn_own_fee_refused(&creating, ProverAccount::Missing, Timestamp::ZERO));
    assert!(!pool.warn_own_fee_refused(
        &low,
        ProverAccount::Present { can_receive: true },
        Timestamp::ZERO
    ));
}
