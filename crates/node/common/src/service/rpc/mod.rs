//! # RPC Service Layer
//!
//! This module provides the service layer that connects HTTP endpoints to the
//! node's state machine. It handles the communication between external RPC
//! requests and the internal event-driven architecture.
//!
//! ## Architecture
//!
//! The RPC system follows a request-response pattern through channels:
//!
//! ```text
//! HTTP Server -> RpcSender -> RpcService -> Event -> State Machine
//!                                                         |
//! HTTP Response <- oneshot channel <- respond_* methods <-+
//! ```
//!
//! 1. HTTP server receives a request and sends it via [`RpcSender`]
//! 2. [`RpcService`] wraps the request with a unique ID and responder channel
//! 3. Request becomes an [`Event::Rpc`] dispatched to the state machine
//! 4. State machine processes the request and calls a `respond_*` method
//! 5. Response is sent back through the oneshot channel to the HTTP server
//!
//! ## Adding a new RPC endpoint
//!
//! To add a new RPC endpoint, follow these steps:
//!
//! 1. **Define the request type** in [`mina_node::rpc`]:
//!    Add a variant to `RpcRequest` enum in `node/src/rpc/mod.rs`
//!
//! 2. **Define the response type** in [`mina_node::rpc`]:
//!    Create a type alias like `pub type RpcMyNewResponse = ...;`
//!
//! 3. **Add the respond method** to `RpcService` trait in
//!    `node/src/rpc_effectful/mod.rs`:
//!    ```ignore
//!    fn respond_my_new_endpoint(
//!        &mut self,
//!        rpc_id: RpcId,
//!        response: RpcMyNewResponse,
//!    ) -> Result<(), RespondError>;
//!    ```
//!
//! 4. **Implement the respond method** in this file using the macro:
//!    ```ignore
//!    rpc_service_impl!(respond_my_new_endpoint, RpcMyNewResponse);
//!    ```
//!
//! 5. **Handle the request** in `node/src/rpc/rpc_reducer.rs`:
//!    Add a match arm for your new `RpcRequest` variant
//!
//! 6. **Add the HTTP route** in `node/native/src/http_server.rs`:
//!    Define the route and call `rpc_sender.my_new_endpoint()`
//!
//! 7. **Add the sender method** in `sender.rs`:
//!    Implement the method on [`RpcSender`] that sends the request through the
//!    channel
//!
//! 8. **Document the endpoint** in the website documentation:
//!    Add the new endpoint to `website/docs/developers/api-and-data/rpc-api.md`
//!    so node operators can discover and use it

mod interleave;
pub use interleave::RpcEventInterleave;
mod sender;
mod state_filter;
pub use sender::RpcSender;

pub mod ledger;
pub mod state;
pub mod stats;
pub mod transaction_pool;
pub mod transition_frontier;

use mina_node::rpc::{
    RpcBestChainResponse, RpcBlockProducerStatsGetResponse, RpcConsensusConstantsGetResponse,
    RpcConsensusTimeGetResponse, RpcDiscoveryBoostrapStatsResponse,
    RpcDiscoveryRoutingTableResponse, RpcGenesisBlockResponse, RpcGetBlockResponse,
    RpcHardForkExportResponse, RpcHealthCheckResponse, RpcHeartbeatGetResponse,
    RpcLedgerAccountDelegatorsGetResponse, RpcLedgerAccountsResponse, RpcLedgerExportResponse,
    RpcLedgerSlimAccountsResponse, RpcLedgerStatusGetResponse, RpcLightAccountResponse,
    RpcLightBestTipResponse, RpcMessageProgressResponse, RpcPeersGetResponse,
    RpcPooledUserCommandsResponse, RpcPooledZkappCommandsResponse, RpcReadinessCheckResponse,
    RpcRequest, RpcSnarkPoolCompletedJobsResponse, RpcSnarkPoolPendingJobsGetResponse,
    RpcStatusGetResponse, RpcTransactionInjectResponse, RpcTransactionPoolResponse,
    RpcTransactionStatusGetResponse, RpcTransitionFrontierUserCommandsResponse,
    RpcVerificationStatsGetResponse,
};
use serde::{Deserialize, Serialize};

use mina_node::{
    core::{
        channels::{mpsc, oneshot},
        requests::PendingRequests,
    },
    event_source::Event,
    p2p::connection::P2pConnectionResponse,
    rpc::RpcSnarkPoolJobGetResponse,
    State,
};
pub use mina_node::{
    rpc::{
        ActionStatsResponse, RpcActionStatsGetResponse, RpcId, RpcIdType,
        RpcP2pConnectionOutgoingResponse, RpcScanStateSummaryGetResponse, RpcSnarkPoolGetResponse,
        RpcSnarkerJobCommitResponse, RpcSnarkerJobSpecResponse, RpcStateGetResponse,
        RpcSyncStatsGetResponse, RpcTransactionInjectSuccess,
    },
    rpc_effectful::RespondError,
};

use crate::{EventReceiver, NodeService};

#[derive(Serialize, Deserialize, Debug)]
pub enum RpcP2pConnectionIncomingResponse {
    Answer(P2pConnectionResponse),
    Result(Result<(), String>),
}

pub struct NodeRpcRequest {
    pub req: RpcRequest,
    pub responder: Box<dyn Send + std::any::Any>,
}

pub type RpcReceiver = mpsc::Receiver<NodeRpcRequest>;

pub struct RpcService {
    pending: PendingRequests<RpcIdType, Box<dyn Send + std::any::Any>>,

    req_sender: mpsc::Sender<NodeRpcRequest>,
    req_receiver: mpsc::Receiver<NodeRpcRequest>,
    interleave: RpcEventInterleave,
}

impl Default for RpcService {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcService {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(8);
        Self {
            pending: Default::default(),
            req_sender: tx,
            req_receiver: rx,
            interleave: Default::default(),
        }
    }

    /// The next event for `ProcessEvents`: a pending RPC request or the next
    /// queued event, see [`RpcEventInterleave`]. `now` is the time of the
    /// pick.
    pub fn next_event(&mut self, now: redux::Instant, events: &mut EventReceiver) -> Option<Event> {
        let Self {
            pending,
            req_receiver,
            interleave,
            ..
        } = self;
        interleave.next(
            now,
            || {
                let req = req_receiver.try_recv().ok()?;
                Some(Event::Rpc(pending.add(req.responder), Box::new(req.req)))
            },
            || events.try_next(),
        )
    }

    /// The run loop found the event queue empty, see
    /// [`RpcEventInterleave::events_drained`].
    pub fn events_drained(&mut self) {
        self.interleave.events_drained();
    }

    /// Channel for sending the rpc request to state machine.
    pub fn req_sender(&self) -> RpcSender {
        RpcSender::new(self.req_sender.clone())
    }

    /// Channel for receiving rpc requests in state machine.
    pub fn req_receiver(&mut self) -> &mut RpcReceiver {
        &mut self.req_receiver
    }

    pub fn process_request(&mut self, req: NodeRpcRequest) -> Event {
        let rpc_id = self.pending.add(req.responder);
        let req = req.req;
        Event::Rpc(rpc_id, Box::new(req))
    }
}

impl NodeService {
    pub fn process_rpc_request(&mut self, req: NodeRpcRequest) {
        let rpc_id = self.rpc.pending.add(req.responder);
        let req = req.req;
        let tx = self.event_sender.clone();

        let _ = tx.send(Event::Rpc(rpc_id, Box::new(req)));
    }
}

macro_rules! rpc_service_impl {
    ($name:ident, $ty:ty) => {
        fn $name(&mut self, rpc_id: RpcId, response: $ty) -> Result<(), RespondError> {
            let entry = self.rpc.pending.remove(rpc_id);
            let chan = entry.ok_or(RespondError::UnknownRpcId)?;
            let chan = chan
                .downcast::<oneshot::Sender<$ty>>()
                .map_err(|_| RespondError::UnexpectedResponseType)?;
            chan.send(response)
                .map_err(|_| RespondError::RespondingFailed)?;
            Ok(())
        }
    };
}

macro_rules! state_field_filter {
    ($state:expr, $($part:ident)|*, $filter:expr ) => {
        $(
            if let Some(filter) = strip_root_field($filter, stringify!($part)) {
                (serde_json::to_value(&$state.$part)?, format!("${filter}"))
            } else
        )*
        {
            (serde_json::to_value($state)?, $filter.to_string())
        }
    };
}

/// Strips topmost field name `field` from the jsonpath expression `filter`,
/// returning modified filter. If the `filter` does not start with the specified
/// field, returns [None].
///
/// ```ignore
/// use mina_node_native::rpc::strip_root_field;
///
/// let filter = strip_root_field("$.field", "field");
/// assert_eq!(filter, Some(""));
///
/// let filter = strip_root_field("$.field.another", "field");
/// assert_eq!(filter, Some(".another"));
///
/// let filter = strip_root_field("$.field_other", "field");
/// assert_eq!(filter, None);
/// ```
fn strip_root_field<'a>(filter: &'a str, field: &str) -> Option<&'a str> {
    let strip_root = |f: &'a str| f.strip_prefix('$');
    let field_char = |c: char| c.is_alphabetic() || c == '_';
    let strip_dot_field = |f: &'a str| {
        f.strip_prefix('.').and_then(|f| {
            f.strip_prefix(field)
                .and_then(|f| (!f.starts_with(field_char)).then_some(f))
        })
    };
    let strip_index_field = |f: &'a str| {
        f.strip_prefix("['")
            .and_then(|f| f.strip_prefix(field))
            .and_then(|f| f.strip_prefix("']"))
    };
    strip_root(filter).and_then(|f| strip_dot_field(f).or_else(|| strip_index_field(f)))
}

fn optimize_filtered_state(
    state: &State,
    filter: &str,
) -> Result<(serde_json::Value, String), serde_json::Error> {
    let (value, filter) = state_field_filter!(
        state,
        config
            | p2p
            | snark
            | transition_frontier
            | snark_pool
            | external_snark_worker
            | block_producer
            | rpc
            | watched_accounts,
        filter
    );
    Ok((value, filter))
}

impl mina_node::rpc_effectful::RpcService for NodeService {
    fn respond_memory_debug(
        &mut self,
        rpc_id: RpcId,
        state: State,
        stats: Option<mina_node::stats::Stats>,
    ) -> Result<(), RespondError> {
        let entry = self.rpc.pending.remove(rpc_id);
        let chan = entry.ok_or(RespondError::UnknownRpcId)?;
        let chan = chan
            .downcast::<oneshot::Sender<serde_json::Value>>()
            .or(Err(RespondError::UnexpectedResponseType))?;
        let ledger = self.ledger_manager.memory_debug_handle();
        std::thread::Builder::new()
            .name("memory-diagnostic".into())
            .spawn(move || {
                let state_debug =
                    mina_node::rpc_effectful::memory_debug(&state, stats.as_ref(), ledger.read());
                let response = serde_json::json!({
                    "state": state_debug,
                    "p2p_heap_bytes": mina_node::rpc_effectful::memory_debug_p2p_heap(&state),
                    "frontier_counts": state.transition_frontier.resources_usage(),
                    "snark_pool_counts": state.snark_pool.resources_usage(),
                });
                let _ = chan.send(response);
            })
            .map_err(|err| RespondError::Custom(err.to_string()))?;
        Ok(())
    }

    fn respond_state_get(
        &mut self,
        rpc_id: RpcId,
        (state, filter): (&State, Option<&str>),
    ) -> Result<(), RespondError> {
        let entry = self.rpc.pending.remove(rpc_id);
        let chan = entry.ok_or(RespondError::UnknownRpcId)?;
        let chan = chan
            .downcast::<oneshot::Sender<RpcStateGetResponse>>()
            .or(Err(RespondError::UnexpectedResponseType))?;
        let response = if let Some(filter) = filter {
            // Validate before serializing even a top-level state subtree.
            match state_filter::StateFilter::parse(filter) {
                Err(err) => Err(err),
                Ok(_) => {
                    let (json_state, filter) = optimize_filtered_state(state, filter)?;
                    state_filter::StateFilter::parse(&filter)
                        .and_then(|filter| filter.evaluate(&json_state))
                }
            }
        } else {
            Ok(serde_json::to_value(state)?)
        };
        chan.send(response)
            .or(Err(RespondError::RespondingFailed))?;
        Ok(())
    }
    rpc_service_impl!(respond_status_get, RpcStatusGetResponse);
    rpc_service_impl!(respond_metrics_get, mina_node::rpc::RpcMetricsGetResponse);
    rpc_service_impl!(
        respond_snark_work_coverage_get,
        mina_node::snark_pool::WorkCoverageStatus
    );

    rpc_service_impl!(respond_heartbeat_get, RpcHeartbeatGetResponse);
    rpc_service_impl!(
        respond_proof_memory_get,
        mina_node::rpc::RpcProofMemoryGetResponse
    );

    rpc_service_impl!(respond_sync_stats_get, RpcSyncStatsGetResponse);
    rpc_service_impl!(respond_action_stats_get, RpcActionStatsGetResponse);
    rpc_service_impl!(
        respond_verification_stats_get,
        RpcVerificationStatsGetResponse
    );
    rpc_service_impl!(
        respond_block_producer_stats_get,
        RpcBlockProducerStatsGetResponse
    );
    rpc_service_impl!(
        respond_message_progress_stats_get,
        RpcMessageProgressResponse
    );
    rpc_service_impl!(respond_peers_get, RpcPeersGetResponse);
    rpc_service_impl!(
        respond_p2p_connection_outgoing,
        RpcP2pConnectionOutgoingResponse
    );

    fn respond_p2p_connection_incoming_answer(
        &mut self,
        rpc_id: RpcId,
        response: P2pConnectionResponse,
    ) -> Result<(), RespondError> {
        let entry = self.rpc.pending.get(rpc_id);
        let chan = entry.ok_or(RespondError::UnknownRpcId)?;
        let chan = chan
            .downcast_ref::<mpsc::Sender<RpcP2pConnectionIncomingResponse>>()
            .ok_or(RespondError::UnexpectedResponseType)?
            .clone();
        chan.try_send(RpcP2pConnectionIncomingResponse::Answer(response))
            .or(Err(RespondError::RespondingFailed))?;
        Ok(())
    }

    fn respond_p2p_connection_incoming(
        &mut self,
        rpc_id: RpcId,
        response: Result<(), String>,
    ) -> Result<(), RespondError> {
        let entry = self.rpc.pending.remove(rpc_id);
        let chan = entry.ok_or(RespondError::UnknownRpcId)?;
        let chan = chan
            .downcast::<mpsc::Sender<RpcP2pConnectionIncomingResponse>>()
            .or(Err(RespondError::UnexpectedResponseType))?;
        chan.try_send(RpcP2pConnectionIncomingResponse::Result(response))
            .or(Err(RespondError::RespondingFailed))?;
        Ok(())
    }

    rpc_service_impl!(
        respond_scan_state_summary_get,
        RpcScanStateSummaryGetResponse
    );
    rpc_service_impl!(respond_snark_pool_get, RpcSnarkPoolGetResponse);
    rpc_service_impl!(respond_snark_pool_job_get, RpcSnarkPoolJobGetResponse);
    rpc_service_impl!(
        respond_snark_pool_completed_jobs_get,
        RpcSnarkPoolCompletedJobsResponse
    );
    rpc_service_impl!(
        respond_snark_pool_pending_jobs_get,
        RpcSnarkPoolPendingJobsGetResponse
    );
    rpc_service_impl!(respond_snarker_job_commit, RpcSnarkerJobCommitResponse);
    rpc_service_impl!(
        respond_snarker_job_spec,
        mina_node::rpc::RpcSnarkerJobSpecResponse
    );
    rpc_service_impl!(
        respond_snarker_workers,
        mina_node::rpc::RpcSnarkerWorkersResponse
    );
    rpc_service_impl!(
        respond_snarker_config_get,
        mina_node::rpc::RpcSnarkerConfigGetResponse
    );
    rpc_service_impl!(
        respond_snark_worker_control_get,
        mina_node::rpc::RpcSnarkWorkerControlGetResponse
    );
    rpc_service_impl!(
        respond_snark_worker_control_set,
        mina_node::rpc::RpcSnarkWorkerControlSetResponse
    );
    rpc_service_impl!(
        respond_snark_work_included_get,
        mina_node::rpc::RpcSnarkWorkIncludedGetResponse
    );
    rpc_service_impl!(respond_health_check, RpcHealthCheckResponse);
    rpc_service_impl!(respond_readiness_check, RpcReadinessCheckResponse);
    rpc_service_impl!(
        respond_discovery_routing_table,
        RpcDiscoveryRoutingTableResponse
    );
    rpc_service_impl!(
        respond_discovery_bootstrap_stats,
        RpcDiscoveryBoostrapStatsResponse
    );
    rpc_service_impl!(respond_transaction_pool, RpcTransactionPoolResponse);
    rpc_service_impl!(respond_ledger_slim_accounts, RpcLedgerSlimAccountsResponse);
    rpc_service_impl!(respond_ledger_accounts, RpcLedgerAccountsResponse);
    rpc_service_impl!(respond_transaction_inject, RpcTransactionInjectResponse);
    rpc_service_impl!(
        respond_transition_frontier_commands,
        RpcTransitionFrontierUserCommandsResponse
    );
    rpc_service_impl!(respond_best_chain, RpcBestChainResponse);
    rpc_service_impl!(
        respond_consensus_constants,
        RpcConsensusConstantsGetResponse
    );
    rpc_service_impl!(respond_transaction_status, RpcTransactionStatusGetResponse);
    rpc_service_impl!(
        respond_transaction_nonce,
        mina_node::rpc::RpcTransactionNonceGetResponse
    );
    rpc_service_impl!(respond_block_get, RpcGetBlockResponse);
    rpc_service_impl!(respond_pooled_user_commands, RpcPooledUserCommandsResponse);
    rpc_service_impl!(
        respond_pooled_zkapp_commands,
        RpcPooledZkappCommandsResponse
    );
    rpc_service_impl!(respond_genesis_block, RpcGenesisBlockResponse);
    rpc_service_impl!(respond_consensus_time_get, RpcConsensusTimeGetResponse);
    rpc_service_impl!(respond_ledger_status_get, RpcLedgerStatusGetResponse);
    rpc_service_impl!(
        respond_ledger_account_delegators_get,
        RpcLedgerAccountDelegatorsGetResponse
    );
    rpc_service_impl!(respond_ledger_export, RpcLedgerExportResponse);
    rpc_service_impl!(respond_hard_fork_config, RpcHardForkExportResponse);
    rpc_service_impl!(respond_light_best_tip, RpcLightBestTipResponse);
    rpc_service_impl!(respond_light_account, RpcLightAccountResponse);
}

#[cfg(test)]
mod tests {
    use super::strip_root_field;

    #[test]
    fn strip_root_field_test() {
        for (filter, expected) in [
            ("$.field", Some("")),
            ("$['field']", Some("")),
            ("$.field.another", Some(".another")),
            ("$['field'].another", Some(".another")),
            ("$.another", None),
            ("$.field_1", None),
            ("$.fields", None),
        ] {
            let actual = strip_root_field(filter, "field");
            assert_eq!(actual, expected)
        }
    }
}
