use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::P2pTimeouts;

use super::{P2pRpcId, P2pRpcKind, P2pRpcRequest};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pChannelsRpcState {
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
        local: P2pRpcLocalState,
        /// We are the responders here.
        remote: P2pRpcRemoteState,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pRpcLocalState {
    WaitingForRequest {
        time: redux::Timestamp,
    },
    Requested {
        time: redux::Timestamp,
        id: P2pRpcId,
        request: Box<P2pRpcRequest>,
    },
    Responded {
        time: redux::Timestamp,
        id: P2pRpcId,
        request: Box<P2pRpcRequest>,
    },
    /// The request timed out once, at `time`, and whoever made it has been
    /// told, to retry it with another peer. The peer is kept: its answer may
    /// still come, and moves us to `Responded`. No new request is sent to it
    /// meanwhile, so that a late answer can't be taken for the answer to
    /// another request. If the request times out a second time, the peer is
    /// disconnected: a timeout after `time` for a libp2p peer, and for a WebRTC
    /// peer at [`crate::channels::stretched_timeout`] after `sent`
    /// ([`crate::channels::timed_out_request_expired`]).
    TimedOut {
        time: redux::Timestamp,
        /// When the request was sent.
        sent: redux::Timestamp,
        id: P2pRpcId,
        request: Box<P2pRpcRequest>,
    },
}

impl P2pRpcLocalState {
    /// Whether a new request may be sent: none of ours is unanswered.
    pub fn can_send_request(&self) -> bool {
        matches!(
            self,
            P2pRpcLocalState::WaitingForRequest { .. } | P2pRpcLocalState::Responded { .. }
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pRpcRemoteState {
    pub pending_requests: VecDeque<P2pRpcRemotePendingRequestState>,
    pub last_responded: redux::Timestamp,
}

static EMPTY_REMOTE_REQUESTS: VecDeque<P2pRpcRemotePendingRequestState> = VecDeque::new();

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pRpcRemotePendingRequestState {
    pub time: redux::Timestamp,
    pub id: P2pRpcId,
    pub request: P2pRpcRequest,
    /// If a given rpc request requires a response from some async component,
    /// e.g. ledger, then this field will be set to `true` once request
    /// to that async component is initiated.
    pub is_pending: bool,
}

impl P2pChannelsRpcState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn can_send_request(&self) -> bool {
        match self {
            Self::Ready { local, .. } => local.can_send_request(),
            _ => false,
        }
    }

    /// Whether our request `rpc_id` has gone unanswered for its timeout: since
    /// it was sent, or since its first timeout if it has had one. For a WebRTC
    /// peer, `last_received` makes it count from the peer's last bytes
    /// ([`crate::channels::request_timed_out`]).
    pub fn is_timed_out(
        &self,
        rpc_id: P2pRpcId,
        now: redux::Timestamp,
        config: &P2pTimeouts,
        last_received: Option<redux::Timestamp>,
    ) -> bool {
        match self {
            Self::Ready {
                local: P2pRpcLocalState::Requested { time, id, request },
                ..
            } => {
                rpc_id == *id
                    && request.kind().timeout(config).is_some_and(|timeout| {
                        crate::channels::request_timed_out(*time, timeout, now, last_received)
                    })
            }
            Self::Ready {
                local:
                    P2pRpcLocalState::TimedOut {
                        time,
                        sent,
                        id,
                        request,
                    },
                ..
            } => {
                rpc_id == *id
                    && request.kind().timeout(config).is_some_and(|timeout| {
                        crate::channels::timed_out_request_expired(
                            *sent,
                            *time,
                            timeout,
                            now,
                            last_received,
                        )
                    })
            }
            _ => false,
        }
    }

    pub fn pending_local_rpc_id(&self) -> Option<P2pRpcId> {
        match self {
            Self::Ready {
                local: P2pRpcLocalState::Requested { id, .. },
                ..
            } => Some(*id),
            _ => None,
        }
    }

    /// Our request that is still unanswered, whether or not it has timed out
    /// once.
    pub fn unanswered_local_rpc(&self) -> Option<(P2pRpcId, &P2pRpcRequest)> {
        match self {
            Self::Ready {
                local:
                    P2pRpcLocalState::Requested { id, request, .. }
                    | P2pRpcLocalState::TimedOut { id, request, .. },
                ..
            } => Some((*id, request)),
            _ => None,
        }
    }

    /// Our request that timed out once, if the peer hasn't answered it since.
    pub fn timed_out_local_rpc(&self) -> Option<(P2pRpcId, &P2pRpcRequest)> {
        match self {
            Self::Ready {
                local: P2pRpcLocalState::TimedOut { id, request, .. },
                ..
            } => Some((*id, request)),
            _ => None,
        }
    }

    pub fn pending_local_rpc(&self) -> Option<&P2pRpcRequest> {
        match self {
            Self::Ready {
                local: P2pRpcLocalState::Requested { request, .. },
                ..
            } => Some(request),
            _ => None,
        }
    }

    pub fn pending_local_rpc_kind(&self) -> Option<P2pRpcKind> {
        self.pending_local_rpc().map(|req| req.kind())
    }

    pub fn local_responded_request(&self) -> Option<(P2pRpcId, &P2pRpcRequest)> {
        match self {
            Self::Ready {
                local: P2pRpcLocalState::Responded { id, request, .. },
                ..
            } => Some((*id, request)),
            _ => None,
        }
    }

    fn remote_requests(&self) -> impl Iterator<Item = &P2pRpcRemotePendingRequestState> {
        match self {
            Self::Ready { remote, .. } => remote.pending_requests.iter(),
            _ => EMPTY_REMOTE_REQUESTS.iter(),
        }
    }

    pub fn remote_todo_requests_iter(
        &self,
    ) -> impl Iterator<Item = &P2pRpcRemotePendingRequestState> {
        self.remote_requests().filter(|req| !req.is_pending)
    }

    pub fn remote_pending_requests_iter(
        &self,
    ) -> impl Iterator<Item = &P2pRpcRemotePendingRequestState> {
        self.remote_requests().filter(|req| req.is_pending)
    }

    pub fn remote_last_responded(&self) -> redux::Timestamp {
        match self {
            Self::Ready { remote, .. } => remote.last_responded,
            _ => redux::Timestamp::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(secs: u64) -> redux::Timestamp {
        redux::Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    fn ready(local: P2pRpcLocalState) -> P2pChannelsRpcState {
        P2pChannelsRpcState::Ready {
            time: at(0),
            local,
            remote: P2pRpcRemoteState {
                pending_requests: Default::default(),
                last_responded: redux::Timestamp::ZERO,
            },
        }
    }

    fn timeouts() -> P2pTimeouts {
        P2pTimeouts {
            best_tip_with_proof: Some(Duration::from_secs(15)),
            ..P2pTimeouts::default()
        }
    }

    fn request() -> Box<P2pRpcRequest> {
        Box::new(P2pRpcRequest::BestTipWithProof)
    }

    /// After its first timeout a request blocks the peer's slot, may still be
    /// answered, and times out again a full timeout later.
    #[test]
    fn a_timed_out_request_keeps_the_slot_until_answered_or_timed_out_again() {
        let config = timeouts();
        let requested = ready(P2pRpcLocalState::Requested {
            time: at(10),
            id: 7,
            request: request(),
        });
        assert!(!requested.can_send_request());
        assert!(!requested.is_timed_out(7, at(24), &config, None));
        assert!(requested.is_timed_out(7, at(25), &config, None));

        // The first timeout, at 25 s.
        let timed_out = ready(P2pRpcLocalState::TimedOut {
            time: at(25),
            sent: at(10),
            id: 7,
            request: request(),
        });
        assert!(!timed_out.can_send_request());
        assert_eq!(timed_out.pending_local_rpc_id(), None);
        assert_eq!(timed_out.unanswered_local_rpc().map(|(id, _)| id), Some(7));
        assert_eq!(timed_out.timed_out_local_rpc().map(|(id, _)| id), Some(7));
        assert!(!timed_out.is_timed_out(7, at(39), &config, None));
        assert!(timed_out.is_timed_out(7, at(40), &config, None));
        assert!(!timed_out.is_timed_out(8, at(40), &config, None));
        // From a WebRTC peer the second timeout is the ceiling counted from
        // the send at 10 s, 150 s for a 15 s timeout, whatever it sent since.
        for last_received in [at(0), at(100), at(159)] {
            let last_received = Some(last_received);
            assert!(!timed_out.is_timed_out(7, at(159), &config, last_received));
            assert!(timed_out.is_timed_out(7, at(160), &config, last_received));
        }

        // Answered late, the slot is free again.
        let responded = ready(P2pRpcLocalState::Responded {
            time: at(30),
            id: 7,
            request: request(),
        });
        assert!(responded.can_send_request());
        assert_eq!(responded.unanswered_local_rpc(), None);
    }
}
