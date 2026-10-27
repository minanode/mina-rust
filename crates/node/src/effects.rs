use mina_core::log::system_time;

use crate::{
    block_producer::{BlockProducerAction, BlockProducerWonSlotDiscardReason},
    block_producer_effectful::block_producer_effects,
    event_source::event_source_effects,
    external_snark_worker_effectful::external_snark_worker_effectful_effects,
    ledger::read::LedgerReadAction,
    ledger_effectful::ledger_effectful_effects,
    logger::logger_effects,
    p2p::node_p2p_effects,
    p2p_ready,
    rpc_effectful::rpc_effects,
    snark::snark_effects,
    snark_pool::{candidate::SnarkPoolCandidateAction, snark_pool_effects, SnarkPoolAction},
    transaction_pool::candidate::TransactionPoolCandidateAction,
    transition_frontier::{
        candidate::TransitionFrontierCandidateAction, genesis::TransitionFrontierGenesisAction,
        persist::TransitionFrontierPersistAction, transition_frontier_effects,
        TransitionFrontierAction,
    },
    Action, ActionWithMeta, CheckInvalidPeersAction, ExternalSnarkWorkerAction, Service, Store,
    TransactionPoolAction,
};

use crate::p2p::channels::rpc::{P2pChannelsRpcAction, P2pRpcRequest};

pub fn effects<S: Service>(store: &mut Store<S>, action: ActionWithMeta) {
    store.service.recorder().action(&action);

    let (action, meta) = action.split();

    if let Some(stats) = store.service.stats() {
        stats.new_action(action.kind(), meta.clone());
    }

    logger_effects(store, meta.clone().with_action(&action));
    match action {
        // Following action gets dispatched very often, so ideally this
        // effect execution should be as light as possible.
        Action::CheckTimeouts(_) => {
            if let Some(stats) = store.service.stats() {
                stats
                    .block_producer()
                    .resolve_restored_schedules(meta.time());
            }
            store.service.save_block_production();
            // TODO(binier): create init action and dispatch these there.
            store.dispatch(TransitionFrontierGenesisAction::LedgerLoadInit);
            store.dispatch(ExternalSnarkWorkerAction::Start);

            store.dispatch(TransitionFrontierGenesisAction::ProveInit);

            // The frontier store (R20 D4, D5): opens once the chain id is
            // known, and is loaded for a restore once the genesis block is in
            // the frontier; completed work renews its idle deadline. A
            // checkpoint starts when one is due and the ledger is free.
            store.dispatch(TransitionFrontierPersistAction::OpenInit);
            store.dispatch(TransitionFrontierPersistAction::OpenTimeout);
            store.dispatch(TransitionFrontierPersistAction::LoadInit);
            let completed_stages = store.service.persist_load_progress();
            store.dispatch(TransitionFrontierPersistAction::LoadProgress { completed_stages });
            store.dispatch(TransitionFrontierPersistAction::LoadTimeout);
            store.dispatch(TransitionFrontierPersistAction::ChainPutInit);
            store.dispatch(TransitionFrontierPersistAction::CheckpointInit);
            // Genesis ledgers deferred at startup, if the node bootstraps
            // after all (I5d2).
            store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit);

            if store.state().p2p.ready().is_some() {
                // Within the best tip budget, and once per peer per candidate.
                store.dispatch(TransitionFrontierCandidateAction::ChainProofPeersQuery);
                p2p_request_best_tips_of_new_peers(store);
                p2p_request_transactions_if_needed(store);
                p2p_request_snarks_if_needed(store);
            }

            store.dispatch(TransactionPoolAction::P2pSendAll);
            store.dispatch(TransactionPoolCandidateAction::FetchAll);
            store.dispatch(TransactionPoolCandidateAction::VerifyNext);

            store.dispatch(SnarkPoolAction::CheckTimeouts);
            // A worker held back for one of our blocks takes a job again as
            // soon as it may (P5.3).
            if store.state().snark_pool.worker_paused_for_block
                && store.state().snark_worker_may_take_job()
            {
                store.dispatch(SnarkPoolAction::AutoCreateCommitment);
            }
            store.dispatch(SnarkPoolAction::P2pSendAll);

            store.dispatch(SnarkPoolAction::CompletedSnarksFetch);
            store.dispatch(SnarkPoolCandidateAction::WorkFetchAll);
            store.dispatch(SnarkPoolCandidateAction::WorkVerifyNext);

            store.dispatch(ExternalSnarkWorkerAction::StartTimeout { now: meta.time() });
            store.dispatch(ExternalSnarkWorkerAction::WorkTimeout { now: meta.time() });

            store.dispatch(BlockProducerAction::WonSlotDiscard {
                reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
            });
            store.dispatch(BlockProducerAction::WonSlotProduceInit);
            store.dispatch(BlockProducerAction::BlockInject);
            store.dispatch(LedgerReadAction::FindTodos);
            store.dispatch(SnarkPoolAction::WorkCoverageRefresh);

            store.dispatch(CheckInvalidPeersAction {});
        }
        Action::EventSource(action) => {
            event_source_effects(store, meta.with_action(action));
        }
        Action::Snark(action) => {
            snark_effects(store, meta.with_action(action));
        }
        Action::TransactionPoolEffect(action) => {
            // At a start the frontier is synced to the genesis block before
            // the genesis ledger is loaded (RC7-FIXES).
            let frontier = &store.state().transition_frontier;
            let synced = frontier.sync.is_synced()
                && frontier.best_tip().is_some_and(|tip| !tip.is_genesis());
            action.effects(store, synced);
        }
        Action::TransitionFrontier(action) => {
            transition_frontier_effects(store, meta.with_action(action));
        }
        Action::P2pEffectful(action) => {
            node_p2p_effects(store, meta.with_action(action));
        }
        Action::LedgerEffects(action) => {
            ledger_effectful_effects(store, meta.with_action(action));
        }
        Action::SnarkPoolEffect(action) => {
            snark_pool_effects(store, meta.with_action(action));
        }
        Action::BlockProducerEffectful(action) => {
            block_producer_effects(store, meta.with_action(action));
        }
        Action::ExternalSnarkWorkerEffects(action) => {
            external_snark_worker_effectful_effects(store, meta.with_action(action));
        }
        Action::RpcEffectful(action) => {
            rpc_effects(store, meta.with_action(action));
        }
        Action::CheckInvalidPeersAction(_) => {
            let state = store.state();
            let initial_peers = state
                .p2p
                .config()
                .initial_peers
                .iter()
                .map(|opts| opts.to_string())
                .collect::<Vec<_>>()
                .join(", ");

            crate::core::error!(
                crate::core::log::system_time();
                error = "Invalid initial peers",
                initial_peers = initial_peers
            );

            #[cfg(feature = "exit-on-invalid-peers")]
            {
                crate::core::error!(
                    crate::core::log::system_time();
                    summary = "Exiting due to invalid peers",
                );
                std::process::exit(1);
            }
        }
        Action::BlockProducer(_)
        | Action::SnarkPool(_)
        | Action::ExternalSnarkWorker(_)
        | Action::TransactionPool(_)
        | Action::Ledger(_)
        | Action::Rpc(_)
        | Action::WatchedAccounts(_)
        | Action::P2pCallbacks(_)
        | Action::P2p(_) => {
            // Handled by reducer
        }
    }
}

/// Ask new peers, and refresh unusable bootstrap hints within the shared
/// peer/IP request budget. A libp2p best-tip channel does not push updates.
/// OCaml downloads and verifies best tips during initialization; the periodic
/// refresh here is a local recovery policy for quiet fresh-peer chains.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/mina_networking.ml#L232-L256>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_router/transition_router.ml#L186-L266>
fn p2p_request_best_tips_of_new_peers<S: Service>(store: &mut Store<S>) {
    let requests = bootstrap_best_tip_requests(store.state());

    for (peer_id, id) in requests {
        store.dispatch(P2pChannelsRpcAction::RequestSend {
            peer_id,
            id,
            request: Box::new(P2pRpcRequest::BestTipWithProof),
            on_init: None,
        });
    }
}

pub(crate) fn bootstrap_best_tip_requests(
    state: &crate::State,
) -> Vec<(crate::p2p::PeerId, crate::p2p::channels::rpc::P2pRpcId)> {
    let time = state.time();
    let Some(p2p) = state.p2p.ready() else {
        return Vec::new();
    };
    let frontier = &state.transition_frontier;
    let no_target = frontier.best_tip().is_none_or(|tip| tip.is_genesis())
        && frontier.sync.best_tip().is_none_or(|tip| tip.is_genesis())
        && frontier
            .candidates
            .best_verified_block()
            .is_none_or(|tip| tip.is_genesis());
    let stale_hint = |tip: &mina_core::block::ArcBlockWithHash| {
        tip.is_genesis() || crate::transition_frontier::sync::tip_is_stale(tip, time)
    };
    let mut requests = p2p
        .peers
        .iter()
        .filter(|(_, peer)| peer.is_libp2p())
        .filter_map(|(peer_id, peer)| Some((peer_id, peer.status.as_ready()?)))
        .filter(|(peer_id, peer)| {
            let new_peer = peer.best_tip.is_none()
                && p2p
                    .best_tip_requests
                    .last_request(peer_id)
                    .is_none_or(|last| last < peer.connected_since);
            let refresh = no_target && peer.best_tip.as_ref().is_none_or(stale_hint);
            (new_peer || refresh)
                && peer.channels.rpc.can_send_request()
                && p2p.is_best_tip_request_within_budget(peer_id, time)
        })
        .map(|(peer_id, peer)| (*peer_id, peer.channels.next_local_rpc_id()))
        .collect::<Vec<_>>();
    // A shared-IP budget must not let one genesis-only peer monopolize all
    // refreshes. RequestSend rechecks the budget after each actual dispatch.
    requests.sort_by_key(|(peer, _)| p2p.best_tip_requests.last_request(peer));
    requests
}

fn p2p_request_transactions_if_needed<S: Service>(store: &mut Store<S>) {
    use crate::p2p::channels::transaction::P2pChannelsTransactionAction;

    const MAX_PEER_PENDING_TXS: usize = 32;

    let state = store.state();
    let p2p = p2p_ready!(
        state.p2p,
        "p2p_request_transactions_if_needed",
        system_time()
    );
    let transaction_reqs = p2p
        .ready_peers_iter()
        .filter(|(_, p)| p.channels.transaction.can_send_request())
        .map(|(peer_id, _)| {
            let pending_txs = state
                .transaction_pool
                .candidates
                .peer_transaction_count(peer_id);
            (peer_id, MAX_PEER_PENDING_TXS.saturating_sub(pending_txs))
        })
        .filter(|(_, limit)| *limit > 0)
        .map(|(peer_id, limit)| (*peer_id, limit.min(u8::MAX as usize) as u8))
        .collect::<Vec<_>>();

    for (peer_id, limit) in transaction_reqs {
        store.dispatch(P2pChannelsTransactionAction::RequestSend { peer_id, limit });
    }
}

fn p2p_request_snarks_if_needed<S: Service>(store: &mut Store<S>) {
    use crate::p2p::channels::snark::P2pChannelsSnarkAction;

    const MAX_PEER_PENDING_SNARKS: usize = 32;

    let state = store.state();
    let p2p = p2p_ready!(state.p2p, "p2p_request_snarks_if_needed", system_time());
    let snark_reqs = p2p
        .ready_peers_iter()
        .filter(|(_, p)| p.channels.snark.can_send_request())
        .map(|(peer_id, _)| {
            let pending_snarks = state.snark_pool.candidates.peer_work_count(peer_id);
            (
                peer_id,
                MAX_PEER_PENDING_SNARKS.saturating_sub(pending_snarks),
            )
        })
        .filter(|(_, limit)| *limit > 0)
        .map(|(peer_id, limit)| (*peer_id, limit.min(u8::MAX as usize) as u8))
        .collect::<Vec<_>>();

    for (peer_id, limit) in snark_reqs {
        store.dispatch(P2pChannelsSnarkAction::RequestSend { peer_id, limit });
    }
}
