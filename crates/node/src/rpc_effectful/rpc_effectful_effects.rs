use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    time::Duration,
};

use super::{super::rpc, RpcEffectfulAction};
use crate::{
    block_producer::BlockProducerWonSlot,
    external_snark_worker::available_job_to_snark_worker_spec,
    p2p::{
        channels::streaming_rpc::P2pStreamingRpcReceiveProgress, connection::P2pConnectionResponse,
    },
    p2p_ready,
    rpc::{
        AccountQuery, AccountSlim, ActionStatsQuery, ActionStatsResponse, CurrentMessageProgress,
        MessagesStats, NodeHeartbeat, ProducedBlockInfo, RootLedgerSyncProgress,
        RootStagedLedgerSyncProgress, RpcAction, RpcBlockProducerStats, RpcMessageProgressResponse,
        RpcNextBlockProduction, RpcNodeStatus, RpcNodeStatusLedger, RpcNodeStatusNetworkInfo,
        RpcNodeStatusResources, RpcNodeStatusTransactionPool, RpcNodeStatusTransitionFrontier,
        RpcNodeStatusTransitionFrontierBlockSummary, RpcNodeStatusTransitionFrontierSync,
        RpcRequestExtraData, RpcScanStateSummary, RpcScanStateSummaryBlock,
        RpcScanStateSummaryBlockTransaction, RpcScanStateSummaryBlockTransactionKind,
        RpcScanStateSummaryScanStateJob, RpcSnarkPoolJobFull, RpcSnarkPoolJobSnarkWork,
        RpcSnarkPoolJobSummary, RpcSnarkerJobCommitResponse, RpcSnarkerJobSpecResponse,
        RpcTransactionInjectResponse, TransactionStatus,
    },
    snark_pool::SnarkPoolAction,
    transition_frontier::sync::{
        ledger::TransitionFrontierSyncLedgerState, TransitionFrontierSyncState,
    },
    Service, Store,
};
use ledger::{
    scan_state::currency::{Balance, Magnitude},
    Account,
};
use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};
use mina_core::{block::ArcBlockWithHash, bug_condition};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::{rpc_kernel::QueryHeader, v2};
use mina_signer::CompressedPubKey;
use redux::ActionWithMeta;

pub fn memory_debug(
    state: &crate::State,
    stats: Option<&crate::stats::Stats>,
    ledger_service: Option<serde_json::Value>,
) -> serde_json::Value {
    static OBSERVED_PEERS: std::sync::OnceLock<std::sync::Mutex<BTreeSet<crate::p2p::PeerId>>> =
        std::sync::OnceLock::new();
    let stats = stats.map(|stats| stats.memory_debug());
    let (ledger_reads_pending, ledger_reads_success) = state.ledger.read.memory_debug_counts();
    let p2p = state.p2p.ready();
    let p2p_parts = p2p.map(|p2p| {
        let observed_peers = OBSERVED_PEERS.get_or_init(Default::default);
        let peers_observed = observed_peers.lock().ok().map(|mut peers| {
            // The diagnostic itself must not retain every peer ID forever.
            for peer in p2p.peers.keys() {
                if peers.len() == 4096 {
                    break;
                }
                peers.insert(peer.clone());
            }
            (peers.len(), peers.len() == 4096)
        });
        let scheduler = &p2p.network.scheduler;
        let measured = |value: &dyn MallocSizeOf| {
            let mut set = BTreeSet::new();
            let fun = move |ptr: *const c_void| !set.insert(ptr.addr());
            let mut ops = MallocSizeOfOps::new(None, Some(Box::new(fun)));
            value.size_of(&mut ops)
        };
        serde_json::json!({
            "peers_heap_bytes": p2p.peers.values().map(|peer| measured(peer)).sum::<usize>(),
            "connections_heap_bytes": scheduler.connections.iter().map(|(addr, conn)| {
                std::mem::size_of_val(addr) + std::mem::size_of_val(conn) + measured(conn)
            }).sum::<usize>(),
            "pubsub_heap_bytes": measured(&scheduler.broadcast_state),
            "pubsub_message_cache_heap_bytes": measured(&scheduler.broadcast_state.mcache),
            "kad_heap_bytes": scheduler.discovery_state.as_ref().map(|s| measured(s)),
            "pubsub_serialized_bytes": crate::stats::serialized_size(&scheduler.broadcast_state),
            "connections_serialized_bytes": crate::stats::serialized_size(&scheduler.connections),
            "kad_serialized_bytes": crate::stats::serialized_size(&scheduler.discovery_state),
            "peers_serialized_bytes": crate::stats::serialized_size(&p2p.peers),
            "peer_table_entries": p2p.peers.len(),
            "peers_observed_ever": peers_observed.map(|(count, _)| count),
            "peers_observed_saturated": peers_observed.map(|(_, saturated)| saturated),
            "dial_addresses": p2p.dial_addresses.len(),
            "connections": scheduler.connections.len(),
            "kad_peers": scheduler.discovery_state.as_ref().map(|s| s.routing_table.buckets.iter().map(|b| b.len()).sum::<usize>()),
            "pubsub_seen": scheduler.broadcast_state.seen.len(),
            "pubsub_message_cache": scheduler.broadcast_state.mcache.map.len(),
            "pubsub_block_sources": scheduler.broadcast_state.block_sources.len(),
            "pubsub_clients": scheduler.broadcast_state.clients.len(),
            "pubsub_iwant": scheduler.broadcast_state.iwant.len(),
            "kad_requests": scheduler.discovery_state.as_ref().map(|s| s.requests.len()),
            "rpc_incoming_peers": scheduler.rpc_incoming_streams.len(),
            "rpc_outgoing_peers": scheduler.rpc_outgoing_streams.len(),
        })
    });
    serde_json::json!({
        "measurement": "streaming JSON bytes are a size proxy, not heap bytes",
        "transition_frontier_serialized_bytes": crate::stats::serialized_size(&state.transition_frontier),
        "snark_pool_serialized_bytes": crate::stats::serialized_size(&state.snark_pool),
        "transaction_pool": state.transaction_pool.memory_debug(),
        "ledger_state_serialized_bytes": crate::stats::serialized_size(&state.ledger),
        "ledger_read_serialized_bytes": crate::stats::serialized_size(&state.ledger.read),
        "ledger_write_serialized_bytes": crate::stats::serialized_size(&state.ledger.write),
        "ledger_write_kind": state.ledger.write.kind().map(|kind| format!("{kind:?}")),
        "ledger_reads_pending": ledger_reads_pending,
        "ledger_reads_success": ledger_reads_success,
        "rpc_state_serialized_bytes": crate::stats::serialized_size(&state.rpc),
        "block_producer_serialized_bytes": crate::stats::serialized_size(&state.block_producer),
        "p2p": p2p_parts,
        "stats": stats,
        "worker_stats": {
            "recent_outcomes": state.external_snark_worker.0.recent.len(),
            "recent_bytes": crate::stats::serialized_size(&state.external_snark_worker.0.recent),
            "included_blocks": state.external_snark_worker.0.included.entries.len(),
            "included_bytes": crate::stats::serialized_size(&state.external_snark_worker.0.included),
        },
        "verification_stats_bytes": crate::stats::serialized_size(&state.verification),
        "ledger_service": ledger_service,
        "pending_rpc_requests": state.rpc.requests.len(),
        "frontier_best_chain_blocks": state.transition_frontier.best_chain.len(),
        "frontier_candidate_blocks": state.transition_frontier.candidates.memory_debug_len(),
        "frontier_root_history_blocks": state.transition_frontier.root_history.iter().count(),
    })
}

pub fn memory_debug_p2p_heap(state: &crate::State) -> usize {
    let mut set = BTreeSet::new();
    let fun = move |ptr: *const c_void| !set.insert(ptr.addr());
    let mut ops = MallocSizeOfOps::new(None, Some(Box::new(fun)));
    size_of_val(&state.p2p).saturating_add(state.p2p.size_of(&mut ops))
}

macro_rules! respond_or_log {
    ($e:expr, $t:expr) => {
        if let Err(err) = $e {
            mina_core::log::warn!($t; "Failed to respond: {err}");
        }
    };
}

pub fn rpc_effects<S: Service>(store: &mut Store<S>, action: ActionWithMeta<RpcEffectfulAction>) {
    let (action, meta) = action.split();

    match action {
        RpcEffectfulAction::GlobalStateGet { rpc_id, filter } => {
            let _ = store
                .service
                .respond_state_get(rpc_id, (store.state.get(), filter.as_deref()));
        }
        RpcEffectfulAction::StatusGet { rpc_id } => {
            let status = compute_node_status(store);
            let _ = store.service.respond_status_get(rpc_id, Some(status));
        }
        RpcEffectfulAction::MemoryDebugGet { rpc_id } => {
            let state = store.state.get().clone();
            let stats = store.service.stats().cloned();
            let _ = store.service.respond_memory_debug(rpc_id, state, stats);
        }
        RpcEffectfulAction::MetricsGet { rpc_id } => {
            let state = store.state.get();
            let mut peers_connecting = 0;
            let mut peers_connected = 0;
            let mut peers_disconnecting = 0;
            let mut peers_disconnected = 0;
            let mut highest_peer_tip_height = None;
            if let Some(p2p) = state.p2p.ready() {
                for peer in p2p.peers.values() {
                    match &peer.status {
                        crate::p2p::P2pPeerStatus::Connecting(_) => peers_connecting += 1,
                        crate::p2p::P2pPeerStatus::Ready(ready) => {
                            peers_connected += 1;
                            if let Some(tip) = &ready.best_tip {
                                highest_peer_tip_height =
                                    Some(highest_peer_tip_height.unwrap_or(0).max(tip.height()));
                            }
                        }
                        crate::p2p::P2pPeerStatus::Disconnecting { .. } => peers_disconnecting += 1,
                        crate::p2p::P2pPeerStatus::Disconnected { .. } => peers_disconnected += 1,
                    }
                }
            }
            let (ledger_sync_fetched, ledger_sync_estimated) = match &state.transition_frontier.sync
            {
                TransitionFrontierSyncState::StakingLedgerPending(pending)
                | TransitionFrontierSyncState::NextEpochLedgerPending(pending)
                | TransitionFrontierSyncState::RootLedgerPending(pending) => {
                    let progress = pending
                        .ledger
                        .snarked()
                        .and_then(|ledger| ledger.estimation());
                    (
                        progress.as_ref().map(|p| p.fetched),
                        progress.map(|p| p.estimation),
                    )
                }
                _ => (None, None),
            };
            let mut snark_pool_jobs = 0;
            let mut snark_pool_completed = 0;
            for job in state.snark_pool.jobs_iter() {
                snark_pool_jobs += 1;
                if job.snark.is_some() {
                    snark_pool_completed += 1;
                }
            }
            let (block_producer_blocks_produced, block_producer_blocks_failed) =
                store.service.stats().map_or((0, 0), |stats| {
                    let stats = stats.block_producer();
                    (stats.blocks_produced, stats.blocks_failed)
                });
            let tip = state.transition_frontier.best_tip();
            let sync_phase = state.reported_sync_phase();
            let response = rpc::RpcMetricsGetResponse {
                chain_id: state.p2p.ready().map(|p2p| p2p.chain_id.to_hex()),
                uptime_secs: u64::from(state.time()).saturating_sub(u64::from(state.start_time()))
                    / 1_000_000_000,
                sync_phase: sync_phase.to_string(),
                ready: rpc::readiness_check(
                    sync_phase,
                    &state.transition_frontier.sync,
                    state.p2p.ready_peers_iter().count(),
                    meta.time(),
                )
                .is_ok(),
                best_tip_height: tip.map(|tip| tip.height()),
                best_tip_global_slot: tip.map(|tip| tip.global_slot()),
                highest_peer_tip_height,
                peers_connecting,
                peers_connected,
                peers_disconnecting,
                peers_disconnected,
                transaction_pool_size: state.transaction_pool.size(),
                transaction_pool_diff_received: state.transaction_pool.telemetry.diffs_received,
                transaction_pool_diff_broadcasted: state
                    .transaction_pool
                    .telemetry
                    .diffs_broadcasted,
                transactions_added_to_pool: state.transaction_pool.telemetry.transactions_added,
                snark_pool_jobs,
                snark_pool_completed,
                snark_pool_diff_received: state.snark_pool.admission_stats.received_gossip,
                snark_pool_diff_broadcasted: state.snark_pool.admission_stats.forwarded,
                block_production_delay_buckets: state.block_production_delay_buckets,
                block_production_delay_sum_ms: state.block_production_delay_sum_ms,
                snark_work_received_total: state.snark_pool.completed_snarks_received,
                verification: state.verification.summary(),
                block_producer_slots_won: state
                    .block_producer
                    .vrf_evaluator()
                    .map_or(0, |vrf| vrf.total_won_slots),
                block_producer_blocks_produced,
                block_producer_blocks_failed,
                snark_worker_proofs_made: state.external_snark_worker.0.proofs_made,
                snark_worker_proofs_failed: state.external_snark_worker.0.proofs_failed,
                snark_worker_proofs_sold: state.external_snark_worker.0.proofs_sold,
                ledger_sync_fetched,
                ledger_sync_estimated,
            };
            let _ = store.service.respond_metrics_get(rpc_id, response);
        }
        RpcEffectfulAction::HeartbeatGet { rpc_id } => {
            let status = compute_node_status(store);
            let last_produced_block = store
                .service
                .stats()
                .and_then(|stats| stats.block_producer().last_produced_block.take());

            let last_produced_block_info = match make_produced_block_info(last_produced_block) {
                Ok(data) => data,
                Err(error) => {
                    bug_condition!(
                        "HeartbeatGet: Failed to encode block header, returning None: {error}"
                    );
                    None
                }
            };

            let heartbeat = NodeHeartbeat {
                status: status.into(),
                node_timestamp: meta.time(),
                peer_id: store.state().p2p.my_id(),
                last_produced_block_info,
            };
            let response = store
                .service()
                .with_producer_keypair(move |sk| heartbeat.sign(sk));

            let _ = store.service.respond_heartbeat_get(rpc_id, response);
        }
        RpcEffectfulAction::ActionStatsGet { rpc_id, query } => match query {
            ActionStatsQuery::SinceStart => {
                let resp = store
                    .service
                    .stats()
                    .map(|s| s.collect_action_stats_since_start())
                    .map(|stats| ActionStatsResponse::SinceStart { stats });
                let _ = store.service.respond_action_stats_get(rpc_id, resp);
            }
            ActionStatsQuery::ForLatestBlock => {
                let resp = store
                    .service
                    .stats()
                    .and_then(|s| s.collect_action_stats_for_block_with_id(None))
                    .map(ActionStatsResponse::ForBlock);
                let _ = store.service.respond_action_stats_get(rpc_id, resp);
            }
            ActionStatsQuery::ForBlockWithId(id) => {
                let resp = store
                    .service
                    .stats()
                    .and_then(|s| s.collect_action_stats_for_block_with_id(Some(id)))
                    .map(ActionStatsResponse::ForBlock);
                let _ = store.service.respond_action_stats_get(rpc_id, resp);
            }
        },
        RpcEffectfulAction::SyncStatsGet { rpc_id, query } => {
            let resp = store
                .service
                .stats()
                .map(|s| s.collect_sync_stats(query.limit));
            let _ = store.service.respond_sync_stats_get(rpc_id, resp);
        }
        RpcEffectfulAction::VerificationStatsGet { rpc_id, query } => {
            let snapshot = store.state().verification.snapshot(query);
            let _ = store
                .service
                .respond_verification_stats_get(rpc_id, snapshot);
        }
        RpcEffectfulAction::ProofMemoryGet { rpc_id } => {
            // Waits for the ledger thread, then walks the state: a
            // diagnostic, served on request only.
            let started = std::time::Instant::now();
            let ledger = store.service.ledger_manager().proof_memory();
            let report =
                crate::proof_memory::ProofMemoryReport::new(store.state(), ledger, started);
            let _ = store.service.respond_proof_memory_get(rpc_id, report);
        }
        RpcEffectfulAction::SnarkWorkCoverageGet { rpc_id } => {
            let state = store.state();
            let mut response = state.snark_pool.work_coverage.clone();
            response.matches_current_tip = state.transition_frontier.sync.is_synced()
                && response.snapshot.as_ref().is_some_and(|snapshot| {
                    state
                        .transition_frontier
                        .best_tip()
                        .is_some_and(|tip| tip.hash() == &snapshot.parent_hash)
                });
            response.fetch_requests = state
                .p2p
                .ready()
                .map_or(0, |p2p| p2p.completed_snarks_requests.sent);
            response.fetched_works = state.snark_pool.completed_snarks_received;
            let _ = store
                .service
                .respond_snark_work_coverage_get(rpc_id, response);
        }
        RpcEffectfulAction::BlockProducerStatsGet { rpc_id } => {
            let mut create_response = || {
                let state = store.state.get();
                let best_tip = state.transition_frontier.best_tip()?;
                let public_key = state.block_producer.config()?.pub_key.clone();
                let won_slots = &state.block_producer.vrf_evaluator()?.won_slots;

                let stats = store.service.stats()?;
                let attempts = stats.block_producer().collect_attempts();
                let future_slot = attempts.last().map_or(0, |v| {
                    v.won_slot.global_slot.checked_add(1).expect("overflow")
                });

                let cur_global_slot = state.cur_global_slot();
                let current_epoch = state.current_epoch();
                let slots_per_epoch = best_tip.constants().slots_per_epoch.as_u32();
                let epoch_start = cur_global_slot.map(|slot| {
                    (slot.checked_div(slots_per_epoch).expect("division by 0"))
                        .checked_mul(slots_per_epoch)
                        .expect("overflow")
                });

                let current_epoch_vrf_stats = current_epoch
                    .and_then(|epoch| stats.block_producer().vrf_evaluator.get(&epoch).cloned());
                let vrf_stats = stats.block_producer().vrf_evaluator.clone();

                Some(RpcBlockProducerStats {
                    current_time: meta.time(),
                    current_global_slot: cur_global_slot,
                    current_epoch,
                    current_epoch_vrf_stats,
                    vrf_stats,
                    epoch_start,
                    epoch_end: epoch_start
                        .map(|slot| slot.checked_add(slots_per_epoch).expect("overflow")),
                    public_key: public_key.into(),
                    attempts,
                    future_won_slots: won_slots
                        .range(future_slot..)
                        .map(|(_, won_slot)| {
                            let won_slot = BlockProducerWonSlot::from_vrf_won_slot(
                                won_slot,
                                best_tip.genesis_timestamp(),
                            );
                            (&won_slot).into()
                        })
                        .collect(),
                })
            };
            let response = create_response();
            let _ = store
                .service
                .respond_block_producer_stats_get(rpc_id, response);
        }
        RpcEffectfulAction::MessageProgressGet { rpc_id } => {
            // TODO: move to stats
            let p2p = p2p_ready!(store.state().p2p, meta.time());
            let messages_stats = p2p
                .network
                .scheduler
                .rpc_outgoing_streams
                .iter()
                .filter_map(|(peer_id, streams)| {
                    let (_, rpc_state) = streams.first_key_value()?;
                    let QueryHeader { tag: name, .. } = rpc_state.pending.clone()?;
                    let name = name.to_string();
                    let buffer = &rpc_state.buffer;
                    let current_request = if buffer.len() < 8 {
                        None
                    } else {
                        let received_bytes = buffer.len().saturating_sub(8);
                        let total_bytes = u64::from_le_bytes(
                            buffer
                                .get(..8)
                                .expect("slice with incorrect length")
                                .try_into()
                                .expect("cannot fail checked above"),
                        ) as usize;
                        Some(CurrentMessageProgress {
                            name,
                            received_bytes,
                            total_bytes,
                        })
                    };

                    Some((
                        *peer_id,
                        MessagesStats {
                            current_request,
                            responses: rpc_state
                                .total_stats
                                .iter()
                                .map(|((name, _), count)| (name.to_string(), *count))
                                .collect(),
                        },
                    ))
                })
                .collect();

            let mut response = RpcMessageProgressResponse {
                messages_stats,
                staking_ledger_sync: None,
                next_epoch_ledger_sync: None,
                root_ledger_sync: None,
            };

            match &store.state().transition_frontier.sync {
                TransitionFrontierSyncState::StakingLedgerPending(state) => {
                    if let TransitionFrontierSyncLedgerState::Snarked(state) = &state.ledger {
                        response.staking_ledger_sync = state.estimation()
                    }
                }
                TransitionFrontierSyncState::NextEpochLedgerPending(state) => {
                    if let TransitionFrontierSyncLedgerState::Snarked(state) = &state.ledger {
                        response.next_epoch_ledger_sync = state.estimation()
                    }
                }
                TransitionFrontierSyncState::RootLedgerPending(state) => match &state.ledger {
                    TransitionFrontierSyncLedgerState::Snarked(state) => {
                        response.root_ledger_sync =
                            state.estimation().map(|data| RootLedgerSyncProgress {
                                fetched: data.fetched,
                                estimation: data.estimation,
                                staged: None,
                            });
                    }
                    TransitionFrontierSyncLedgerState::Staged(state) => {
                        let unknown_staged_progress = || RootStagedLedgerSyncProgress {
                            fetched: 0,
                            total: 1,
                        };
                        let staged = match state.fetch_attempts() {
                            None => state.target_with_parts().map(|(_, parts)| {
                                let v = parts
                                    .map(|parts| parts.total_pieces_to_transfer())
                                    .unwrap_or(0);
                                RootStagedLedgerSyncProgress {
                                    fetched: v,
                                    total: v,
                                }
                            }),
                            Some(attempts) => attempts
                                .iter()
                                .find(|(_, s)| s.fetch_pending_rpc_id().is_some())
                                .map(|(id, _)| id)
                                .and_then(|peer_id| store.state().p2p.get_ready_peer(peer_id))
                                .map(|peer| {
                                    match peer.channels.streaming_rpc.pending_local_rpc_progress() {
                                        None => unknown_staged_progress(),
                                        Some(
                                            P2pStreamingRpcReceiveProgress::StagedLedgerParts(
                                                progress,
                                            ),
                                        ) => {
                                            let (fetched, total) = progress.progress();
                                            RootStagedLedgerSyncProgress { fetched, total }
                                        }
                                    }
                                }),
                        };

                        // We want to answer with a result that will serve as a 100% complete process for the
                        // frontend while it is still waiting for the staged ledger to complete. Could be cleaner.
                        response.root_ledger_sync = Some(RootLedgerSyncProgress {
                            fetched: 1,
                            estimation: 1,
                            staged,
                        });
                    }
                    _ => {}
                },
                _ => {}
            }

            let _ = store
                .service
                .respond_message_progress_stats_get(rpc_id, response);
        }
        RpcEffectfulAction::PeersGet { rpc_id, peers } => {
            respond_or_log!(
                store.service().respond_peers_get(rpc_id, peers),
                meta.time()
            );
        }
        RpcEffectfulAction::P2pConnectionOutgoingError { rpc_id, error } => {
            let _ = store
                .service
                .respond_p2p_connection_outgoing(rpc_id, Err(error));

            store.dispatch(RpcAction::Finish { rpc_id });
        }
        RpcEffectfulAction::P2pConnectionOutgoingSuccess { rpc_id } => {
            let _ = store
                .service
                .respond_p2p_connection_outgoing(rpc_id, Ok(()));
            store.dispatch(RpcAction::Finish { rpc_id });
        }
        RpcEffectfulAction::P2pConnectionIncomingRespond { rpc_id, response } => {
            let error = match &response {
                P2pConnectionResponse::Accepted(_) => None,
                P2pConnectionResponse::Rejected(reason) => Some(format!("Rejected({:?})", reason)),
                P2pConnectionResponse::SignalDecryptionFailed => {
                    Some("RemoteSignalDecryptionFailed".to_owned())
                }
                P2pConnectionResponse::InternalError => Some("RemoteInternalError".to_owned()),
            };
            let _ = store
                .service
                .respond_p2p_connection_incoming_answer(rpc_id, response);
            if let Some(error) = error {
                store.dispatch(RpcAction::P2pConnectionIncomingError { rpc_id, error });
            }
        }
        RpcEffectfulAction::P2pConnectionIncomingError { rpc_id, error } => {
            let _ = store
                .service
                .respond_p2p_connection_incoming(rpc_id, Err(error));
            store.dispatch(RpcAction::Finish { rpc_id });
        }
        RpcEffectfulAction::P2pConnectionIncomingSuccess { rpc_id } => {
            let _ = store
                .service
                .respond_p2p_connection_incoming(rpc_id, Ok(()));
            store.dispatch(RpcAction::Finish { rpc_id });
        }
        RpcEffectfulAction::ScanStateSummaryGetSuccess {
            rpc_id,
            mut scan_state,
        } => {
            let req = store.state().rpc.requests.get(&rpc_id);
            let Some(block) = req.and_then(|req| match &req.data {
                RpcRequestExtraData::FullBlockOpt(opt) => opt.as_ref(),
                _ => None,
            }) else {
                let _ = store.service.respond_scan_state_summary_get(
                    rpc_id,
                    Err("target block not found".to_string()),
                );
                return;
            };
            let coinbases =
                block
                    .coinbase_fee_transfers_iter()
                    .map(|_| RpcScanStateSummaryBlockTransaction {
                        hash: None,
                        kind: RpcScanStateSummaryBlockTransactionKind::Coinbase,
                        status: v2::MinaBaseTransactionStatusStableV2::Applied,
                    });
            let block_summary = RpcScanStateSummaryBlock {
                hash: block.hash().clone(),
                height: block.height(),
                global_slot: block.global_slot_since_genesis(),
                transactions: block
                    .commands_iter()
                    .map(|tx| RpcScanStateSummaryBlockTransaction {
                        hash: tx.data.hash().ok(),
                        kind: (&tx.data).into(),
                        status: tx.status.clone(),
                    })
                    .chain(coinbases)
                    .collect(),
                completed_works: block
                    .completed_works_iter()
                    .map(|work| (&work.proofs).into())
                    .collect(),
            };

            let snark_pool = &store.state().snark_pool;
            scan_state.iter_mut().flatten().flatten().for_each(|job| {
                if let RpcScanStateSummaryScanStateJob::Todo {
                    job_id,
                    bundle_job_id,
                    job: kind,
                    seq_no,
                } = job
                {
                    let Some(data) = snark_pool.get(bundle_job_id) else {
                        return;
                    };
                    let commitment = data.commitment.clone().map(Box::new);
                    let snark = data
                        .snark
                        .as_ref()
                        .map(|snark| RpcSnarkPoolJobSnarkWork {
                            snarker: snark.work.snarker.clone(),
                            fee: snark.work.fee.clone(),
                            received_t: snark.received_t,
                            sender: snark.sender,
                        })
                        .map(Box::new);

                    if commitment.is_none() && snark.is_none() {
                        return;
                    }
                    *job = RpcScanStateSummaryScanStateJob::Pending {
                        job_id: job_id.clone(),
                        bundle_job_id: bundle_job_id.clone(),
                        job: Box::new(kind.clone()),
                        seq_no: *seq_no,
                        commitment,
                        snark,
                    };
                }
            });
            let res = scan_state.map(|scan_state| RpcScanStateSummary {
                block: block_summary,
                scan_state,
            });
            let _ = store.service.respond_scan_state_summary_get(rpc_id, res);
        }
        RpcEffectfulAction::SnarkPoolAvailableJobsGet { rpc_id } => {
            let resp = store
                .state()
                .snark_pool
                .range(..)
                .map(|(_, job)| RpcSnarkPoolJobSummary {
                    time: job.time,
                    id: job.id.clone(),
                    commitment: job.commitment.clone(),
                    snark: job.snark.as_ref().map(|snark| RpcSnarkPoolJobSnarkWork {
                        snarker: snark.work.snarker.clone(),
                        fee: snark.work.fee.clone(),
                        received_t: snark.received_t,
                        sender: snark.sender,
                    }),
                })
                .collect::<Vec<_>>();
            let _ = store.service().respond_snark_pool_get(rpc_id, resp);
        }
        RpcEffectfulAction::SnarkPoolJobGet { job_id, rpc_id } => {
            let resp = store.state().snark_pool.range(..).find_map(|(_, job)| {
                if job.id == job_id {
                    Some(RpcSnarkPoolJobFull {
                        time: job.time,
                        id: job.id.clone(),
                        job: job.job.clone(),
                        commitment: job.commitment.clone(),
                        snark: job.snark.as_ref().map(|snark| RpcSnarkPoolJobSnarkWork {
                            snarker: snark.work.snarker.clone(),
                            fee: snark.work.fee.clone(),
                            received_t: snark.received_t,
                            sender: snark.sender,
                        }),
                    })
                } else {
                    None
                }
            });
            let _ = store.service().respond_snark_pool_job_get(rpc_id, resp);
        }
        RpcEffectfulAction::SnarkPoolCompletedJobsGet { rpc_id, jobs } => {
            respond_or_log!(
                store
                    .service()
                    .respond_snark_pool_completed_jobs_get(rpc_id, jobs),
                meta.time()
            );
        }
        RpcEffectfulAction::SnarkPoolPendingJobsGet { rpc_id, jobs } => {
            respond_or_log!(
                store
                    .service()
                    .respond_snark_pool_pending_jobs_get(rpc_id, jobs),
                meta.time()
            );
        }
        RpcEffectfulAction::SnarkerConfigGet { rpc_id, config } => {
            let _ = store.service().respond_snarker_config_get(rpc_id, config);
        }
        RpcEffectfulAction::SnarkerJobCommit { rpc_id, job_id } => {
            // The gate of `SnarkPoolAction::CommitmentCreate`: our worker
            // proves the job at our configured fee.
            let state = store.state();
            if !state.config.snarker.as_ref().is_some_and(|config| {
                state
                    .snark_pool
                    .should_create_commitment(&job_id, config.fee.0.as_u64())
            }) {
                let _ = store
                    .service()
                    .respond_snarker_job_commit(rpc_id, RpcSnarkerJobCommitResponse::JobNotFound);
                // TODO(binier): differentiate between job not found and job already taken.
                return;
            }
            if !store.state().external_snark_worker.has_idle() {
                let _ = store
                    .service()
                    .respond_snarker_job_commit(rpc_id, RpcSnarkerJobCommitResponse::SnarkerBusy);
                return;
            }
            if store
                .service()
                .respond_snarker_job_commit(rpc_id, RpcSnarkerJobCommitResponse::Ok)
                .is_err()
            {
                return;
            }
            store.dispatch(SnarkPoolAction::CommitmentCreate { job_id });
        }
        RpcEffectfulAction::SnarkerJobSpec { rpc_id, job_id } => {
            let Some(job) = store.state().snark_pool.get(&job_id) else {
                if store
                    .service()
                    .respond_snarker_job_spec(rpc_id, RpcSnarkerJobSpecResponse::JobNotFound)
                    .is_err()
                {
                    return;
                }
                return;
            };
            let input = available_job_to_snark_worker_spec(
                job.job.clone(),
                &store.state().transition_frontier,
            );
            // TODO(binier): maybe don't require snarker to be enabled here.
            let Some(config) = store.state.get().config.snarker.as_ref() else {
                return;
            };
            let public_key = config.public_key.clone().into();
            let fee = config.fee.clone();
            let input = match input {
                    Ok(instances) => RpcSnarkerJobSpecResponse::Ok(
                        Box::new(mina_p2p_messages::v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponse(Some((
                            mina_p2p_messages::v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0 {
                                instances,
                                fee,
                            },
                            public_key,
                        )))
                    )),
                    Err(err) => RpcSnarkerJobSpecResponse::Err(err),
                };

            // TODO: handle potential errors
            let _ = store.service().respond_snarker_job_spec(rpc_id, input);
        }
        RpcEffectfulAction::SnarkerWorkersGet {
            rpc_id,
            snark_worker,
        } => {
            // TODO: handle potential errors
            let _ = store
                .service()
                .respond_snarker_workers(rpc_id, vec![*snark_worker]);
        }
        RpcEffectfulAction::SnarkWorkerControlGet { rpc_id } => {
            let response = store.state().snark_worker_control();
            respond_or_log!(
                store
                    .service()
                    .respond_snark_worker_control_get(rpc_id, response),
                meta.time()
            );
        }
        RpcEffectfulAction::SnarkWorkerControlSet { rpc_id, error } => {
            let response = match error {
                Some(error) => Err(error),
                None => Ok(store.state().snark_worker_control()),
            };
            respond_or_log!(
                store
                    .service()
                    .respond_snark_worker_control_set(rpc_id, response),
                meta.time()
            );
        }
        RpcEffectfulAction::SnarkWorkIncludedGet { rpc_id } => {
            let state = store.state();
            let response = state
                .external_snark_worker
                .included_work()
                .view()
                .with_market(
                    &state.transition_frontier.best_chain,
                    state.snark_pool.completed_snarks_iter(),
                );
            respond_or_log!(
                store
                    .service()
                    .respond_snark_work_included_get(rpc_id, response),
                meta.time()
            );
        }
        RpcEffectfulAction::HealthCheck { rpc_id } => {
            respond_or_log!(
                store
                    .service()
                    .respond_health_check(rpc_id, rpc::health_check()),
                meta.time()
            );
        }
        RpcEffectfulAction::ReadinessCheck { rpc_id } => {
            let state = store.state();
            let ready = rpc::readiness_check(
                state.reported_sync_phase(),
                &state.transition_frontier.sync,
                state.p2p.ready_peers_iter().count(),
                meta.time(),
            );
            respond_or_log!(
                store.service().respond_readiness_check(rpc_id, ready),
                meta.time()
            );
        }
        RpcEffectfulAction::DiscoveryRoutingTable { rpc_id, response } => {
            respond_or_log!(
                store
                    .service()
                    .respond_discovery_routing_table(rpc_id, response),
                meta.time()
            );
        }
        RpcEffectfulAction::DiscoveryBoostrapStats { rpc_id, response } => {
            respond_or_log!(
                store
                    .service()
                    .respond_discovery_bootstrap_stats(rpc_id, response),
                meta.time()
            );
        }
        RpcEffectfulAction::TransactionPool { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_transaction_pool(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::LedgerAccountsGetSuccess {
            rpc_id,
            accounts,
            account_query,
        } => {
            // Is this todo still relevant?
            // TODO(adonagy): maybe something more effective?
            match account_query {
                // all the accounts for the FE in Slim form
                AccountQuery::All => {
                    let mut accounts: BTreeMap<CompressedPubKey, Account> = accounts
                        .into_iter()
                        .map(|acc| (acc.public_key.clone(), acc))
                        .collect();
                    let nonces_and_amount = store
                        .state()
                        .transaction_pool
                        .get_pending_amount_and_nonce();

                    nonces_and_amount
                        .iter()
                        .for_each(|(account_id, (nonce, amount))| {
                            if let Some(account) = accounts.get_mut(&account_id.public_key) {
                                if let Some(nonce) = nonce {
                                    if nonce >= &account.nonce {
                                        // increment the last nonce in the pool
                                        account.nonce = nonce.incr();
                                    }
                                }
                                account.balance = account
                                    .balance
                                    .sub_amount(*amount)
                                    .unwrap_or(Balance::zero());
                            }
                        });

                    let accounts = accounts
                        .into_values()
                        .map(|v| v.into())
                        .collect::<Vec<AccountSlim>>();

                    respond_or_log!(
                        store
                            .service()
                            .respond_ledger_slim_accounts(rpc_id, accounts),
                        meta.time()
                    )
                }
                // for the graphql endpoint
                AccountQuery::SinglePublicKey(..) | AccountQuery::PubKeyWithTokenId(..) => {
                    respond_or_log!(
                        store.service().respond_ledger_accounts(rpc_id, accounts),
                        meta.time()
                    )
                }
                AccountQuery::MultipleIds(..)
                | AccountQuery::TokenOwner(..)
                | AccountQuery::StakingEpochIds(..) => {
                    respond_or_log!(
                        store.service().respond_ledger_accounts(rpc_id, accounts),
                        meta.time()
                    )
                }
            }
        }
        RpcEffectfulAction::TransactionInjectSuccess { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_transaction_inject(
                    rpc_id,
                    RpcTransactionInjectResponse::Success(response)
                ),
                meta.time()
            )
        }
        RpcEffectfulAction::TransactionInjectRejected { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_transaction_inject(
                    rpc_id,
                    RpcTransactionInjectResponse::Rejected(response)
                ),
                meta.time()
            )
        }
        RpcEffectfulAction::TransactionInjectFailure { rpc_id, errors } => {
            respond_or_log!(
                store.service().respond_transaction_inject(
                    rpc_id,
                    RpcTransactionInjectResponse::Failure(errors)
                ),
                meta.time()
            )
        }
        RpcEffectfulAction::TransitionFrontierUserCommandsGet { rpc_id, commands } => {
            respond_or_log!(
                store
                    .service()
                    .respond_transition_frontier_commands(rpc_id, commands),
                meta.time()
            )
        }
        RpcEffectfulAction::BestChain { rpc_id, best_chain } => {
            respond_or_log!(
                store.service().respond_best_chain(rpc_id, best_chain),
                meta.time()
            )
        }
        RpcEffectfulAction::ConsensusConstantsGet { rpc_id, response } => {
            respond_or_log!(
                store
                    .service()
                    .respond_consensus_constants(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::TransactionNonceGet { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_transaction_nonce(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::TransactionStatusGet { rpc_id, tx } => {
            // Check if the transaction is in the pool, if it is, return PENDING
            let tx_hash = tx.hash().ok();

            let in_tx_pool = store
                .state()
                .transaction_pool
                .get_all_transactions()
                .iter()
                .any(|tx_with_hash| Some(&tx_with_hash.hash) == tx_hash.as_ref());

            if in_tx_pool {
                respond_or_log!(
                    store
                        .service()
                        .respond_transaction_status(rpc_id, TransactionStatus::Pending),
                    meta.time()
                );
                return;
            }

            let in_transition_frontier = if let Some(hash) = tx_hash {
                store
                    .state()
                    .transition_frontier
                    .contains_transaction(&hash)
            } else {
                false
            };

            // Check whether the transaction is in the transition frontier, if it is, return INCLUDED
            if in_transition_frontier {
                respond_or_log!(
                    store
                        .service()
                        .respond_transaction_status(rpc_id, TransactionStatus::Included),
                    meta.time()
                )
            // Otherwise, return UNKNOWN
            } else {
                respond_or_log!(
                    store
                        .service()
                        .respond_transaction_status(rpc_id, TransactionStatus::Unknown),
                    meta.time()
                )
            }
        }
        RpcEffectfulAction::BlockGet { rpc_id, block } => {
            respond_or_log!(
                store.service().respond_block_get(rpc_id, block),
                meta.time()
            )
        }

        RpcEffectfulAction::PooledUserCommands {
            rpc_id,
            user_commands,
        } => {
            respond_or_log!(
                store
                    .service()
                    .respond_pooled_user_commands(rpc_id, user_commands),
                meta.time()
            )
        }

        RpcEffectfulAction::PooledZkappCommands {
            rpc_id,
            zkapp_commands,
        } => {
            respond_or_log!(
                store
                    .service()
                    .respond_pooled_zkapp_commands(rpc_id, zkapp_commands),
                meta.time()
            )
        }
        RpcEffectfulAction::GenesisBlock {
            rpc_id,
            genesis_block,
        } => {
            respond_or_log!(
                store.service().respond_genesis_block(rpc_id, genesis_block),
                meta.time()
            )
        }

        RpcEffectfulAction::ConsensusTimeGet {
            rpc_id,
            consensus_time,
        } => {
            respond_or_log!(
                store
                    .service()
                    .respond_consensus_time_get(rpc_id, consensus_time),
                meta.time()
            )
        }
        RpcEffectfulAction::LedgerStatusGetSuccess { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_ledger_status_get(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::LedgerExportSuccess { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_ledger_export(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::LightBestTip { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_light_best_tip(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::LightAccountSuccess { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_light_account(rpc_id, response),
                meta.time()
            );
            // Answered: nothing is left to retry.
            store.dispatch(RpcAction::Finish { rpc_id });
        }
        RpcEffectfulAction::HardForkConfigSuccess { rpc_id, response } => {
            respond_or_log!(
                store.service().respond_hard_fork_config(rpc_id, response),
                meta.time()
            )
        }
        RpcEffectfulAction::LedgerAccountDelegatorsGetSuccess { rpc_id, response } => {
            respond_or_log!(
                store
                    .service()
                    .respond_ledger_account_delegators_get(rpc_id, response),
                meta.time()
            )
        }
    }
}

fn compute_node_status<S: Service>(store: &mut Store<S>) -> RpcNodeStatus {
    let state = store.state.get();
    let chain_id = state.p2p.ready().map(|p2p| p2p.chain_id.to_hex());
    let block_summary = |b: &ArcBlockWithHash| RpcNodeStatusTransitionFrontierBlockSummary {
        timestamp_ms: Some(u64::from(b.timestamp()) / 1_000_000),
        hash: b.hash().clone(),
        height: b.height(),
        global_slot: b.global_slot(),
        timestamp: Some(b.timestamp()),
    };

    let block_production_attempts = store
        .service
        .stats()
        .map_or_else(Vec::new, |stats| stats.block_producer().collect_attempts());

    let current_block_production_attempt = block_production_attempts.last().cloned();

    let previous_block_production_attempt = block_production_attempts
        .len()
        .checked_sub(2)
        .and_then(|idx| block_production_attempts.get(idx))
        .cloned();

    let network_info = RpcNodeStatusNetworkInfo {
        bind_ip: "0.0.0.0".to_string(),
        external_ip: state
            .p2p
            .config()
            .external_addrs
            .first()
            .map(|addr| addr.to_string()),
        client_port: state.config.client_port,
        libp2p_port: state.p2p.config().libp2p_port,
    };

    let block_producer = state
        .block_producer
        .config()
        .map(|config| AccountPublicKey::from(config.pub_key.clone()));
    let coinbase_receiver = state
        .block_producer
        .config()
        .map(|config| AccountPublicKey::from(config.coinbase_receiver().clone()));

    let next_block_production = next_block_production(state);

    let status = RpcNodeStatus {
        network: mina_core::NetworkConfig::global().name.to_owned(),
        uptime_secs: u64::from(state.time()).saturating_sub(u64::from(state.start_time()))
            / 1_000_000_000,
        chain_id,
        block_producer,
        coinbase_receiver,
        next_block_production,
        transition_frontier: RpcNodeStatusTransitionFrontier {
            root: state.transition_frontier.root().map(block_summary),
            best_tip: state.transition_frontier.best_tip().map(block_summary),
            sync: RpcNodeStatusTransitionFrontierSync {
                time: state.transition_frontier.sync.time(),
                status: state.reported_sync_status(),
                phase: state.reported_sync_phase().to_string(),
                target: state.transition_frontier.sync.best_tip().map(block_summary),
            },
        },
        ledger: RpcNodeStatusLedger {
            alive_masks_after_last_commit: state.ledger.alive_masks,
            pending_writes: state
                .ledger
                .write
                .pending_requests()
                .map(|(req, time)| (req.kind(), time))
                .collect(),
            pending_reads: state
                .ledger
                .read
                .pending_requests()
                .map(|(id, req, time)| (id, req.kind(), time))
                .collect(),
        },
        peers: rpc::collect_rpc_peers_info(state),
        snark_pool: state
            .snark_pool
            .jobs_iter()
            .fold(Default::default(), |mut acc, job| {
                if job.snark.is_some() {
                    acc.snarks = acc.snarks.saturating_add(1);
                }
                acc.total_jobs = acc.total_jobs.saturating_add(1);
                acc
            }),
        transaction_pool: RpcNodeStatusTransactionPool {
            transactions: state.transaction_pool.size(),
            transactions_for_propagation: state.transaction_pool.for_propagation_size(),
            transaction_candidates: state.transaction_pool.candidates.transactions_count(),
        },
        current_block_production_attempt,
        previous_block_production_attempt,
        resources_status: RpcNodeStatusResources {
            p2p_malloc_size: {
                let mut set = BTreeSet::new();
                let fun = move |ptr: *const c_void| !set.insert(ptr.addr());
                let mut ops = MallocSizeOfOps::new(None, Some(Box::new(fun)));
                size_of_val(&state.p2p).saturating_add(state.p2p.size_of(&mut ops))
            },
            transition_frontier: state.transition_frontier.resources_usage(),
            snark_pool: state.snark_pool.resources_usage(),
            memory_debug: None,
        },
        service_queues: store.service.queues(),
        network_info,
    };
    status
}

/// The producer records the timing and its source consensus point together.
/// Reading status must not recompute either from a newer best tip.
fn next_block_production(state: &crate::State) -> Option<RpcNextBlockProduction> {
    state.block_producer.next_block_production()
}

fn make_produced_block_info(
    block: Option<ArcBlockWithHash>,
) -> std::io::Result<Option<ProducedBlockInfo>> {
    use base64::{engine::general_purpose::URL_SAFE, Engine as _};
    use mina_p2p_messages::binprot::BinProtWrite;

    let Some(block) = block else { return Ok(None) };

    let height = block.height();
    let global_slot = block.global_slot();
    let hash = block.hash().to_string();
    let mut buf = Vec::with_capacity(5 * 1024 * 1024);
    v2::MinaBlockHeaderStableV2::binprot_write(block.header(), &mut buf)?;

    let base64_encoded_header = URL_SAFE.encode(&buf);

    Ok(Some(ProducedBlockInfo {
        height,
        global_slot,
        hash,
        base64_encoded_header,
    }))
}
