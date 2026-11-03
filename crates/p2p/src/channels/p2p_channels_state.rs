use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    best_tip::P2pChannelsBestTipState,
    rpc::{P2pChannelsRpcState, P2pRpcId, P2pRpcPipeline, P2pRpcRequest, P2pSyncPeerStats},
    signaling::{
        discovery::P2pChannelsSignalingDiscoveryState, exchange::P2pChannelsSignalingExchangeState,
        P2pChannelsSignalingState,
    },
    snark::P2pChannelsSnarkState,
    snark_job_commitment::P2pChannelsSnarkJobCommitmentState,
    streaming_rpc::P2pChannelsStreamingRpcState,
    transaction::P2pChannelsTransactionState,
    ChannelId,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pChannelsState {
    pub signaling: P2pChannelsSignalingState,
    pub best_tip: P2pChannelsBestTipState,
    pub transaction: P2pChannelsTransactionState,
    pub snark: P2pChannelsSnarkState,
    pub snark_job_commitment: P2pChannelsSnarkJobCommitmentState,
    pub rpc: P2pChannelsRpcState,
    /// Ledger queries sent while `rpc`'s request slot is taken (WebRTC).
    #[serde(default)]
    pub rpc_pipeline: P2pRpcPipeline,
    /// How a WebRTC peer answers our sync requests, to rank it for the
    /// sync's bulk requests (`plan_sync_peers`).
    #[serde(default)]
    pub sync_stats: P2pSyncPeerStats,
    pub streaming_rpc: P2pChannelsStreamingRpcState,
    /// Whether the peer said it serves a staged ledger from its last part
    /// and takes a cancelled stream (WebRTC offer or answer), so the sync
    /// may fetch one staged ledger from it and another peer at once.
    #[serde(default)]
    pub staged_ledger_from_last: bool,

    pub(super) next_local_rpc_id: P2pRpcId,
    /// When a WebRTC peer last sent us anything, on any channel: a whole
    /// message, or part of one too long for a chunk. Our requests to it time
    /// out on silence, not on slowness: a far peer's answer can sit behind
    /// megabytes it is still sending us, on a link that does a few hundred
    /// kilobytes a second (WEB-FAR-RELAY-LOOP).
    pub last_received: redux::Timestamp,
}

impl P2pChannelsState {
    pub fn new(enabled_channels: &BTreeSet<ChannelId>) -> Self {
        Self {
            signaling: P2pChannelsSignalingState {
                discovery: match enabled_channels.contains(&ChannelId::SignalingDiscovery) {
                    false => P2pChannelsSignalingDiscoveryState::Disabled,
                    true => P2pChannelsSignalingDiscoveryState::Enabled,
                },
                exchange: match enabled_channels.contains(&ChannelId::SignalingExchange) {
                    false => P2pChannelsSignalingExchangeState::Disabled,
                    true => P2pChannelsSignalingExchangeState::Enabled,
                },
            },
            best_tip: match enabled_channels.contains(&ChannelId::BestTipPropagation) {
                false => P2pChannelsBestTipState::Disabled,
                true => P2pChannelsBestTipState::Enabled,
            },
            snark_job_commitment: match enabled_channels
                .contains(&ChannelId::SnarkJobCommitmentPropagation)
            {
                false => P2pChannelsSnarkJobCommitmentState::Disabled,
                true => P2pChannelsSnarkJobCommitmentState::Enabled,
            },
            transaction: match enabled_channels.contains(&ChannelId::TransactionPropagation) {
                false => P2pChannelsTransactionState::Disabled,
                true => P2pChannelsTransactionState::Enabled,
            },
            snark: match enabled_channels.contains(&ChannelId::SnarkPropagation) {
                false => P2pChannelsSnarkState::Disabled,
                true => P2pChannelsSnarkState::Enabled,
            },
            rpc: match enabled_channels.contains(&ChannelId::Rpc) {
                false => P2pChannelsRpcState::Disabled,
                true => P2pChannelsRpcState::Enabled,
            },
            rpc_pipeline: Default::default(),
            sync_stats: Default::default(),
            streaming_rpc: match enabled_channels.contains(&ChannelId::StreamingRpc) {
                false => P2pChannelsStreamingRpcState::Disabled,
                true => P2pChannelsStreamingRpcState::Enabled,
            },
            staged_ledger_from_last: false,

            next_local_rpc_id: 0,
            last_received: redux::Timestamp::ZERO,
        }
    }

    pub fn next_local_rpc_id(&self) -> P2pRpcId {
        self.next_local_rpc_id
    }

    /// Whether a sync request, a ledger query or a block, may go to the peer
    /// now: in the RPC channel's request slot, or pipelined behind it
    /// ([`P2pRpcPipeline`]).
    pub fn can_send_sync_request(&self) -> bool {
        self.rpc.can_send_request() || self.rpc_pipeline.has_room(&self.rpc)
    }

    /// How many more sync requests the peer takes now: one in a free slot,
    /// and for a WebRTC peer as many as the pipeline has room for.
    pub fn sync_request_room(&self) -> usize {
        let slot = usize::from(self.rpc.can_send_request());
        if !self.rpc_pipeline.enabled {
            return slot;
        }
        let unanswered = self.unanswered_rpc_ids().count();
        let room = super::rpc::MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS.saturating_sub(unanswered);
        if slot == 0 && !self.rpc_pipeline.has_room(&self.rpc) {
            0
        } else {
            room
        }
    }

    /// Whether `request` may go to the peer now.
    pub fn can_send_rpc(&self, request: &P2pRpcRequest) -> bool {
        self.rpc.can_send_request()
            || (P2pRpcPipeline::takes(request) && self.rpc_pipeline.has_room(&self.rpc))
    }

    /// Whether `id` is one of our requests the peer hasn't answered.
    pub fn is_rpc_unanswered(&self, id: P2pRpcId) -> bool {
        self.rpc
            .unanswered_local_rpc()
            .is_some_and(|(rpc_id, _)| rpc_id == id)
            || self.rpc_pipeline.get(id).is_some()
    }

    /// Our requests the peer hasn't answered, in the slot and the pipeline.
    pub fn unanswered_rpc_ids(&self) -> impl Iterator<Item = P2pRpcId> + '_ {
        self.rpc
            .unanswered_local_rpc()
            .map(|(id, _)| id)
            .into_iter()
            .chain(self.rpc_pipeline.requests.iter().map(|r| r.id))
    }

    /// What we asked in request `id`, just answered.
    pub fn rpc_responded_request(&self, id: P2pRpcId) -> Option<&P2pRpcRequest> {
        self.rpc
            .local_responded_request()
            .filter(|(rpc_id, _)| *rpc_id == id)
            .map(|(_, request)| request)
            .or_else(|| self.rpc_pipeline.responded_request(id))
    }

    pub fn rpc_remote_last_responded(&self) -> redux::Timestamp {
        std::cmp::max(
            self.rpc.remote_last_responded(),
            self.streaming_rpc.remote_last_responded(),
        )
    }
}

impl P2pChannelsState {
    pub fn is_channel_ready(&self, chan_id: ChannelId) -> bool {
        match chan_id {
            ChannelId::SignalingDiscovery => self.signaling.discovery.is_ready(),
            ChannelId::SignalingExchange => self.signaling.exchange.is_ready(),
            ChannelId::BestTipPropagation => self.best_tip.is_ready(),
            ChannelId::TransactionPropagation => self.transaction.is_ready(),
            ChannelId::SnarkPropagation => self.snark.is_ready(),
            ChannelId::SnarkJobCommitmentPropagation => self.snark_job_commitment.is_ready(),
            ChannelId::Rpc => self.rpc.is_ready(),
            ChannelId::StreamingRpc => self.rpc.is_ready(),
        }
    }
}
