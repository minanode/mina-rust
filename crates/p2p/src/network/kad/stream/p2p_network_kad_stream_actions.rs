use mina_core::ActionEvent;
use redux::EnablingCondition;
use serde::{Deserialize, Serialize};

use crate::{
    ConnectionAddr, Data, P2pAction, P2pNetworkKademliaRpcReply, P2pNetworkKademliaRpcRequest,
    P2pState, PeerId, StreamId,
};

use super::{
    P2pNetworkKadIncomingStreamState, P2pNetworkKadOutgoingStreamState, P2pNetworkKadStreamState,
    P2pNetworkKademliaStreamWaitOutgoingCallback,
};

/// Kademlia stream related actions.
#[derive(Debug, Clone, Serialize, Deserialize, ActionEvent)]
#[action_event(fields(display(addr), display(peer_id), stream_id, incoming, debug(data)))]
pub enum P2pNetworkKademliaStreamAction {
    /// Creates a new stream state.
    New {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        incoming: bool,
    },

    /// Handles incoming data from the stream.
    IncomingData {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        data: Data,
    },
    /// An incomplete framed message exceeded its receive deadline.
    Timeout {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },
    /// Remote peer sent FIN to close the stream.
    RemoteClose {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },

    /// Reinitializes existing stream state.
    WaitIncoming {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },
    /// Sets the state to wait for outgoing data.
    WaitOutgoing {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        callback: P2pNetworkKademliaStreamWaitOutgoingCallback,
    },

    /// Sends request to the stream.
    SendRequest {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        data: P2pNetworkKademliaRpcRequest,
    },
    /// Sends response to the stream.
    SendResponse {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        data: P2pNetworkKademliaRpcReply,
    },
    /// Outgoing data is ready to be sent via the stream.
    OutgoingDataReady {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },

    /// Start closing outgoing stream (first closing our half of the stream)
    Close {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },

    /// Removes the closed stream from the state.
    #[action_event(level = trace)]
    Prune {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },
}

macro_rules! enum_field {
    ($field:ident: $field_type:ty) => {
        pub fn $field(&self) -> &$field_type {
            match self {
                P2pNetworkKademliaStreamAction::New { $field, .. }
                | P2pNetworkKademliaStreamAction::IncomingData { $field, .. }
                | P2pNetworkKademliaStreamAction::Timeout { $field, .. }
                | P2pNetworkKademliaStreamAction::WaitOutgoing { $field, .. }
                | P2pNetworkKademliaStreamAction::SendRequest { $field, .. }
                | P2pNetworkKademliaStreamAction::SendResponse { $field, .. }
                | P2pNetworkKademliaStreamAction::OutgoingDataReady { $field, .. }
                | P2pNetworkKademliaStreamAction::WaitIncoming { $field, .. }
                | P2pNetworkKademliaStreamAction::Close { $field, .. }
                | P2pNetworkKademliaStreamAction::RemoteClose { $field, .. }
                | P2pNetworkKademliaStreamAction::Prune { $field, .. } => $field,
            }
        }
    };
}

impl P2pNetworkKademliaStreamAction {
    enum_field!(addr: ConnectionAddr);
    enum_field!(peer_id: PeerId);
    enum_field!(stream_id: StreamId);
}

impl EnablingCondition<P2pState> for P2pNetworkKademliaStreamAction {
    fn is_enabled(&self, state: &P2pState, time: redux::Timestamp) -> bool {
        let scheduler = &state.network.scheduler;
        let Some(kad) = scheduler.discovery_state.as_ref() else {
            return false;
        };
        if !matches!(self, Self::Prune { .. })
            && !scheduler
                .connections
                .get(self.addr())
                .is_some_and(|c| c.closed.is_none() && c.peer_id() == Some(self.peer_id()))
        {
            return false;
        }
        let stream = kad.find_kad_stream_state(self.peer_id(), self.stream_id());
        if let Self::Timeout {
            addr,
            peer_id,
            stream_id,
        } = self
        {
            return kad
                .expired_partial_frames(time)
                .contains(&(*addr, *peer_id, *stream_id));
        }
        self.enabled_for(stream)
    }
}

impl P2pNetworkKademliaStreamAction {
    fn enabled_for(&self, stream: Option<&P2pNetworkKadStreamState>) -> bool {
        use P2pNetworkKadIncomingStreamState as I;
        use P2pNetworkKadOutgoingStreamState as O;
        use P2pNetworkKadStreamState::{Incoming, Outgoing};
        match self {
            Self::New { .. } => stream.is_none(),
            Self::IncomingData { .. } => matches!(
                stream,
                Some(
                    Incoming(I::WaitingForRequest { .. } | I::PartialRequestReceived { .. })
                        | Outgoing(O::WaitingForReply | O::PartialReplyReceived { .. })
                )
            ),
            Self::WaitOutgoing { .. } => matches!(
                stream,
                Some(Incoming(I::RequestIsReady { .. }) | Outgoing(O::ResponseIsReady { .. }))
            ),
            Self::WaitIncoming { .. } => matches!(
                stream,
                Some(
                    Incoming(I::ResponseBytesAreReady { .. })
                        | Outgoing(O::RequestBytesAreReady { .. })
                )
            ),
            Self::SendRequest { .. } => matches!(
                stream,
                Some(Outgoing(O::WaitingForRequest {
                    expect_close: false
                }))
            ),
            Self::SendResponse { .. } => matches!(stream, Some(Incoming(I::WaitingForReply))),
            Self::Close { .. } => matches!(
                stream,
                Some(Outgoing(O::WaitingForRequest { expect_close: true }))
            ),
            Self::RemoteClose { .. } => matches!(
                stream,
                Some(Incoming(I::WaitingForRequest { expect_close: true }) | Outgoing(O::Closing))
            ),
            Self::Prune { .. } => matches!(
                stream,
                Some(
                    Incoming(I::Closing | I::Closed | I::Error(_))
                        | Outgoing(O::Closing | O::Closed | O::Error(_))
                )
            ),
            // No reducer implements this legacy action.
            Self::OutgoingDataReady { .. } | Self::Timeout { .. } => false,
        }
    }
}

impl From<P2pNetworkKademliaStreamAction> for P2pAction {
    fn from(value: P2pNetworkKademliaStreamAction) -> Self {
        P2pAction::Network(super::super::P2pNetworkKadAction::Stream(value).into())
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;

    #[test]
    fn kademlia_ordering_matrix() {
        use P2pNetworkKadIncomingStreamState as I;
        use P2pNetworkKadOutgoingStreamState as O;
        use P2pNetworkKadStreamState::{Incoming, Outgoing};
        let peer_id = crate::identity::SecretKey::from_bytes([1; 32])
            .public_key()
            .peer_id();
        let addr = ConnectionAddr {
            sock_addr: "127.0.0.1:8302".parse().unwrap(),
            incoming: true,
        };
        let stream_id = 1;
        let states = [
            None,
            Some(Incoming(I::Default)),
            Some(Outgoing(O::Default)),
            Some(Incoming(I::WaitingForRequest {
                expect_close: false,
            })),
            Some(Incoming(I::WaitingForRequest { expect_close: true })),
            Some(Incoming(I::WaitingForReply)),
            Some(Incoming(I::ResponseBytesAreReady { bytes: vec![] })),
            Some(Incoming(I::Closing)),
            Some(Incoming(I::Closed)),
            Some(Outgoing(O::WaitingForRequest {
                expect_close: false,
            })),
            Some(Outgoing(O::WaitingForRequest { expect_close: true })),
            Some(Outgoing(O::RequestBytesAreReady { bytes: vec![] })),
            Some(Outgoing(O::WaitingForReply)),
            Some(Outgoing(O::Closing)),
            Some(Outgoing(O::Closed)),
            Some(Incoming(I::RequestIsReady {
                data: P2pNetworkKademliaRpcRequest::find_node(peer_id).unwrap(),
            })),
            Some(Outgoing(O::ResponseIsReady {
                data: P2pNetworkKademliaRpcReply::FindNode {
                    closer_peers: vec![],
                },
            })),
            Some(Incoming(I::PartialRequestReceived {
                len: 10,
                data: vec![0],
                since: redux::Timestamp::ZERO,
                addr,
            })),
            Some(Outgoing(O::PartialReplyReceived {
                len: 10,
                data: vec![0],
                since: redux::Timestamp::ZERO,
                addr,
            })),
        ];
        let cases = [
            (
                P2pNetworkKademliaStreamAction::New {
                    addr,
                    peer_id,
                    stream_id,
                    incoming: true,
                },
                vec![0],
            ),
            (
                P2pNetworkKademliaStreamAction::IncomingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::empty(),
                },
                vec![3, 4, 12, 17, 18],
            ),
            (
                P2pNetworkKademliaStreamAction::WaitIncoming {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![6, 11],
            ),
            (
                P2pNetworkKademliaStreamAction::WaitOutgoing {
                    addr,
                    peer_id,
                    stream_id,
                    callback:
                        P2pNetworkKademliaStreamWaitOutgoingCallback::update_find_node_request(
                            vec![],
                        ),
                },
                vec![15, 16],
            ),
            (
                P2pNetworkKademliaStreamAction::SendRequest {
                    addr,
                    peer_id,
                    stream_id,
                    data: P2pNetworkKademliaRpcRequest::FindNode {
                        key: peer_id.try_into().unwrap(),
                    },
                },
                vec![9],
            ),
            (
                P2pNetworkKademliaStreamAction::SendResponse {
                    addr,
                    peer_id,
                    stream_id,
                    data: P2pNetworkKademliaRpcReply::FindNode {
                        closer_peers: vec![],
                    },
                },
                vec![5],
            ),
            (
                P2pNetworkKademliaStreamAction::Close {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![10],
            ),
            (
                P2pNetworkKademliaStreamAction::RemoteClose {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![4, 13],
            ),
            (
                P2pNetworkKademliaStreamAction::Prune {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![7, 8, 13, 14],
            ),
            (
                P2pNetworkKademliaStreamAction::OutgoingDataReady {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![],
            ),
            (
                P2pNetworkKademliaStreamAction::Timeout {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![],
            ),
        ];
        for (action, allowed) in cases {
            for (i, state) in states.iter().enumerate() {
                assert_eq!(
                    action.enabled_for(state.as_ref()),
                    allowed.contains(&i),
                    "{action:?} in {state:?}"
                );
            }
        }
    }
}
