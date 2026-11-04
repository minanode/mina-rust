use serde::{Deserialize, Serialize};

use crate::P2pTimeouts;

use super::{
    staged_ledger_parts::{StagedLedgerPartsFromLastProgress, StagedLedgerPartsReceiveProgress},
    P2pStreamingRpcId, P2pStreamingRpcKind, P2pStreamingRpcReceiveProgress, P2pStreamingRpcRequest,
    P2pStreamingRpcResponse, P2pStreamingRpcResponseFull, P2pStreamingRpcSendProgress,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "Happy-path variant is the redux state"
)]
pub enum P2pChannelsStreamingRpcState {
    Disabled,
    Enabled,
    Init {
        time: redux::Timestamp,
    },
    Pending {
        time: redux::Timestamp,
    },
    Ready {
        time: redux::Timestamp,
        /// We are the requestors here.
        local: P2pStreamingRpcLocalState,
        /// We are the responders here.
        remote: P2pStreamingRpcRemoteState,
        remote_last_responded: redux::Timestamp,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pStreamingRpcLocalState {
    WaitingForRequest {
        time: redux::Timestamp,
    },
    Requested {
        time: redux::Timestamp,
        id: P2pStreamingRpcId,
        request: Box<P2pStreamingRpcRequest>,
        progress: P2pStreamingRpcReceiveProgress,
    },
    Responded {
        time: redux::Timestamp,
        id: P2pStreamingRpcId,
        request: Box<P2pStreamingRpcRequest>,
    },
    /// We cancelled the request while a part was on its way: it may still
    /// come, and is dropped. No new request goes before it.
    Cancelled {
        time: redux::Timestamp,
        id: P2pStreamingRpcId,
        request: Box<P2pStreamingRpcRequest>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pStreamingRpcRemoteState {
    WaitingForRequest {
        time: redux::Timestamp,
    },
    Requested {
        time: redux::Timestamp,
        id: P2pStreamingRpcId,
        request: Box<P2pStreamingRpcRequest>,
        progress: P2pStreamingRpcSendProgress,
    },
    Responded {
        time: redux::Timestamp,
        id: P2pStreamingRpcId,
        request: Box<P2pStreamingRpcRequest>,
    },
}

impl P2pChannelsStreamingRpcState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn can_send_request(&self) -> bool {
        match self {
            Self::Ready { local, .. } => matches!(
                local,
                P2pStreamingRpcLocalState::WaitingForRequest { .. }
                    | P2pStreamingRpcLocalState::Responded { .. }
            ),
            _ => false,
        }
    }

    /// Whether the part we asked for last has not come within the timeout.
    /// For a WebRTC peer, `last_received` makes it count from the peer's
    /// last bytes ([`crate::channels::request_timed_out`]): a part of
    /// megabytes on a slow link is still arriving.
    pub fn is_timed_out(
        &self,
        rpc_id: P2pStreamingRpcId,
        now: redux::Timestamp,
        config: &P2pTimeouts,
        last_received: Option<redux::Timestamp>,
    ) -> bool {
        match self {
            Self::Ready {
                local:
                    P2pStreamingRpcLocalState::Requested {
                        id,
                        request,
                        progress,
                        ..
                    },
                ..
            } => {
                rpc_id == *id
                    && request.kind().timeout(config).is_some_and(|timeout| {
                        crate::channels::request_timed_out(
                            progress.last_updated(),
                            timeout,
                            now,
                            last_received,
                        )
                    })
            }
            _ => false,
        }
    }

    pub fn pending_local_rpc_id(&self) -> Option<P2pStreamingRpcId> {
        match self {
            Self::Ready {
                local: P2pStreamingRpcLocalState::Requested { id, .. },
                ..
            } => Some(*id),
            _ => None,
        }
    }

    pub fn pending_local_rpc(&self) -> Option<&P2pStreamingRpcRequest> {
        match self {
            Self::Ready {
                local: P2pStreamingRpcLocalState::Requested { request, .. },
                ..
            } => Some(request),
            _ => None,
        }
    }

    pub fn pending_local_rpc_kind(&self) -> Option<P2pStreamingRpcKind> {
        self.pending_local_rpc().map(|req| req.kind())
    }

    pub fn pending_local_rpc_progress(&self) -> Option<&P2pStreamingRpcReceiveProgress> {
        match self {
            Self::Ready {
                local: P2pStreamingRpcLocalState::Requested { progress, .. },
                ..
            } => Some(progress),
            _ => None,
        }
    }

    pub(super) fn local_done_response(&self) -> Option<P2pStreamingRpcResponseFull> {
        match self {
            Self::Ready {
                local:
                    P2pStreamingRpcLocalState::Requested {
                        progress:
                            P2pStreamingRpcReceiveProgress::StagedLedgerParts(
                                StagedLedgerPartsReceiveProgress::Success { data, .. },
                            ),
                        ..
                    },
                ..
            } => Some(data.clone().into()),
            Self::Ready {
                local:
                    P2pStreamingRpcLocalState::Requested {
                        progress:
                            P2pStreamingRpcReceiveProgress::StagedLedgerPartsFromLast(
                                StagedLedgerPartsFromLastProgress {
                                    data: Some(data), ..
                                },
                            ),
                        ..
                    },
                ..
            } => Some(data.clone().into()),
            _ => None,
        }
    }

    pub fn local_responded_request(&self) -> Option<(P2pStreamingRpcId, &P2pStreamingRpcRequest)> {
        match self {
            Self::Ready {
                local: P2pStreamingRpcLocalState::Responded { id, request, .. },
                ..
            } => Some((*id, request)),
            _ => None,
        }
    }

    #[allow(unused)]
    pub(super) fn remote_request(&self) -> Option<&P2pStreamingRpcRequest> {
        match self {
            Self::Ready {
                remote: P2pStreamingRpcRemoteState::Requested { request, .. },
                ..
            } => Some(request),
            _ => None,
        }
    }

    pub fn remote_todo_request(&self) -> Option<(P2pStreamingRpcId, &P2pStreamingRpcRequest)> {
        match self {
            Self::Ready {
                remote:
                    P2pStreamingRpcRemoteState::Requested {
                        id,
                        request,
                        progress,
                        ..
                    },
                ..
            } if progress.external_data_todo() => Some((*id, request)),
            _ => None,
        }
    }

    pub fn remote_pending_request(&self) -> Option<(P2pStreamingRpcId, &P2pStreamingRpcRequest)> {
        match self {
            Self::Ready {
                remote:
                    P2pStreamingRpcRemoteState::Requested {
                        id,
                        request,
                        progress,
                        ..
                    },
                ..
            } if progress.external_data_pending() => Some((*id, request)),
            _ => None,
        }
    }

    pub fn remote_next_msg(&self) -> Option<P2pStreamingRpcResponse> {
        match self {
            Self::Ready {
                remote: P2pStreamingRpcRemoteState::Requested { progress, .. },
                ..
            } => progress.next_msg(),
            _ => None,
        }
    }

    pub fn remote_last_responded(&self) -> redux::Timestamp {
        match self {
            Self::Ready {
                remote_last_responded,
                ..
            } => *remote_last_responded,
            _ => redux::Timestamp::ZERO,
        }
    }
}
