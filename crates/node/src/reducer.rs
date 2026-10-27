use crate::p2p::{P2pAction, P2pEffectfulAction, P2pInitializeAction, P2pState};
use mina_core::{bug_condition, error, Substate};

use crate::{
    external_snark_worker::ExternalSnarkWorkers,
    rpc::RpcState,
    state::{BlockProducerState, LedgerState},
    transition_frontier::{
        candidate::TransitionFrontierCandidateAction,
        sync::ledger::{
            snarked::TransitionFrontierSyncLedgerSnarkedAction,
            staged::TransitionFrontierSyncLedgerStagedAction,
        },
    },
    Action, ActionWithMeta, EventSourceAction, P2p, State,
};

pub fn reducer(
    state: &mut State,
    action: &ActionWithMeta,
    dispatcher: &mut redux::Dispatcher<Action, State>,
) {
    let meta = action.meta().clone();
    let time = meta.time();
    match action.action() {
        Action::CheckTimeouts(_) => {
            if state.p2p.ready().is_some() {
                if let Err(error) =
                    P2pState::p2p_timeout_dispatch(Substate::new(state, dispatcher), &meta)
                {
                    bug_condition!("{}", error);
                };
            }
            // A refusal can leave every query waiting for backoff, with no
            // outstanding RPC to wake either ledger scheduler.
            // LN2: light requests held back past their deadline.
            dispatcher.push(crate::rpc::RpcAction::LightAccountsExpire);
            dispatcher.push(TransitionFrontierSyncLedgerSnarkedAction::PeersQuery);
            dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
            dispatcher.push(
                crate::transition_frontier::sync::TransitionFrontierSyncAction::BlocksPeersQuery,
            );
            dispatcher.push(TransitionFrontierCandidateAction::TransitionFrontierSyncTargetUpdate);
        }
        Action::EventSource(EventSourceAction::NewEvent { .. }) => {}
        Action::EventSource(_) => {}
        Action::P2p(a) => match a {
            P2pAction::Initialization(P2pInitializeAction::Initialize { chain_id }) => {
                if let Err(err) = state.p2p.initialize(chain_id) {
                    error!(meta.time(); summary = "error initializing p2p", error = display(err));
                }
                dispatcher.push(P2pEffectfulAction::Initialize);
            }
            p2p_action => match &mut state.p2p {
                P2p::Pending(_) => {
                    error!(meta.time(); summary = "p2p is not initialized", action = debug(p2p_action))
                }
                P2p::Ready(_) => {
                    let time = meta.time();
                    let result = crate::p2p::P2pState::reducer(
                        Substate::new(state, dispatcher),
                        meta.with_action(p2p_action.clone()),
                    );

                    if let Err(error) = result {
                        use crate::ActionKindGet as _;
                        error!(time;
                            summary = "Failure when handling a P2P action",
                            action_kind = format!("{}", p2p_action.kind()),
                            error = display(error));
                    }
                }
            },
        },
        Action::P2pEffectful(_) => {}
        Action::Ledger(action) => {
            LedgerState::reducer(Substate::new(state, dispatcher), meta.with_action(action));
        }
        Action::LedgerEffects(_) => {}
        Action::Snark(a) => {
            if let crate::snark::SnarkAction::BlockVerify(
                crate::snark::block_verify::SnarkBlockVerifyAction::Init { block, .. },
            ) = a
            {
                state.verification.record(
                    &crate::verification::VerificationUpdate::Proof {
                        block: crate::verification::BlockIdentity::header(
                            block.hash_ref().clone(),
                            block.header_ref(),
                        ),
                        status: crate::verification::CheckStatus::Pending,
                        elapsed_ms: None,
                        verifier_key: None,
                    },
                    meta.time(),
                );
            }
            crate::snark::SnarkState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::TransitionFrontier(a) => {
            if let crate::TransitionFrontierAction::VerificationRecorded { update } = a {
                state.verification.record(update, meta.time());
                state.action_applied(action);
                return;
            }
            let skip_proof_verification = state.config.skip_proof_verification;
            crate::transition_frontier::TransitionFrontierState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
                skip_proof_verification,
            );
        }
        Action::SnarkPool(a) => {
            crate::snark_pool::SnarkPoolState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::SnarkPoolEffect(_) => {}
        Action::TransactionPool(a) => {
            crate::transaction_pool::TransactionPoolState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::TransactionPoolEffect(_) => {}
        Action::BlockProducer(action) => {
            let slot_start = matches!(action, crate::BlockProducerAction::BlockProduced)
                .then(|| {
                    state
                        .block_producer
                        .current_won_slot()
                        .map(|slot| slot.slot_time)
                })
                .flatten();
            BlockProducerState::reducer(state, meta.with_action(action), dispatcher);
            if let Some(slot_start) = slot_start {
                if state.block_producer.produced_block().is_some() {
                    // OCaml observes the delay when the block is produced,
                    // before submitting it to the frontier.
                    // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L1539-L1550
                    let delay_ms =
                        u64::from(time).saturating_sub(u64::from(slot_start)) / 1_000_000;
                    let bucket = [60_000, 90_000, 120_000, 150_000, 180_000, 210_000]
                        .into_iter()
                        .position(|limit| delay_ms <= limit)
                        .unwrap_or(6);
                    state.block_production_delay_buckets[bucket] =
                        state.block_production_delay_buckets[bucket].saturating_add(1);
                    state.block_production_delay_sum_ms =
                        state.block_production_delay_sum_ms.saturating_add(delay_ms);
                }
            }
        }
        Action::BlockProducerEffectful(_) => {}
        Action::ExternalSnarkWorker(action) => {
            if let crate::ExternalSnarkWorkerAction::Configure { setting } = action {
                state.snark_worker_configure(setting.as_ref(), time, dispatcher);
            }
            ExternalSnarkWorkers::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(action),
            );
        }
        Action::ExternalSnarkWorkerEffects(_) => {}
        Action::Rpc(action) => {
            RpcState::reducer(Substate::new(state, dispatcher), meta.with_action(action));
        }
        Action::RpcEffectful(_) => {}
        Action::WatchedAccounts(a) => {
            crate::watched_accounts::WatchedAccountsState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::P2pCallbacks(action) => {
            State::p2p_callback_reducer(Substate::new(state, dispatcher), meta.with_action(action))
        }
        Action::CheckInvalidPeersAction(_) => {}
    }

    // Observe successful application separately from proof verification. The
    // root reconstructed during bootstrap was not applied individually.
    if let Action::TransitionFrontier(crate::TransitionFrontierAction::Synced { .. }) =
        action.action()
    {
        for block in &state.transition_frontier.best_chain {
            state
                .verification
                .applied(block.block_with_hash(), time, false);
        }
        state.verification.has_synced = true;

        // Our included work (P5.4): the worker's key is looked for from
        // the first block on; the log keeps every key it has used.
        let now_ms = u64::from(time) / 1_000_000;
        let State {
            config,
            transition_frontier,
            snark_pool,
            external_snark_worker,
            ..
        } = &mut *state;
        let log = &mut external_snark_worker.0.included;
        if let Some(snarker) = &config.snarker {
            log.watch(&snarker.public_key, now_ms);
        }
        log.on_best_chain(
            &transition_frontier.best_chain,
            transition_frontier
                .root_history
                .iter()
                .map(|block| (block.hash(), block.height())),
            |job_id| {
                snark_pool
                    .get(job_id)
                    .is_some_and(|job| job.has_zkapp_command())
            },
        );
    }
    if let Action::Ledger(crate::ledger::LedgerAction::Write(
        crate::ledger::write::LedgerWriteAction::Success {
            response:
                crate::ledger::write::LedgerWriteResponse::BlockApply {
                    result: Ok(result), ..
                },
        },
    )) = action.action()
    {
        let newly_applied = state.verification.applied(&result.block, time, true);
        if let Some(config) = state.config.snarker.as_ref().filter(|_| newly_applied) {
            let prover: mina_p2p_messages::v2::NonZeroCurvePoint = config.public_key.clone().into();
            let sold = result
                .block
                .completed_works_iter()
                .filter(|work| work.prover == prover)
                .count() as u64;
            state.external_snark_worker.0.proofs_sold = state
                .external_snark_worker
                .0
                .proofs_sold
                .saturating_add(sold);
        }
    }

    // must be the last.
    state.action_applied(action);
}
