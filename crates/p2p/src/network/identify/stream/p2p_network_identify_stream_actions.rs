use super::P2pNetworkIdentifyStreamState;
use crate::{ConnectionAddr, Data, P2pAction, P2pState, PeerId, StreamId};
use mina_core::ActionEvent;
use multiaddr::Multiaddr;
use redux::EnablingCondition;
use serde::{Deserialize, Serialize};

/// Identify stream related actions.
#[derive(Debug, Clone, Serialize, Deserialize, ActionEvent)]
pub enum P2pNetworkIdentifyStreamAction {
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
    /// Start closing the stream (send FIN).
    Close {
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
    /// Removes the closed stream from the state.
    Prune {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
    },
    SendIdentify {
        addr: ConnectionAddr,
        peer_id: PeerId,
        stream_id: StreamId,
        addresses: Vec<Multiaddr>,
    },
}

macro_rules! enum_field {
    ($field:ident: $field_type:ty) => {
        pub fn $field(&self) -> &$field_type {
            match self {
                P2pNetworkIdentifyStreamAction::New { $field, .. }
                | P2pNetworkIdentifyStreamAction::IncomingData { $field, .. }
                | P2pNetworkIdentifyStreamAction::Close { $field, .. }
                | P2pNetworkIdentifyStreamAction::RemoteClose { $field, .. }
                | P2pNetworkIdentifyStreamAction::SendIdentify { $field, .. }
                | P2pNetworkIdentifyStreamAction::Prune { $field, .. } => $field,
            }
        }
    };
}

impl P2pNetworkIdentifyStreamAction {
    enum_field!(addr: ConnectionAddr);
    enum_field!(peer_id: PeerId);
    enum_field!(stream_id: StreamId);
}

impl EnablingCondition<P2pState> for P2pNetworkIdentifyStreamAction {
    fn is_enabled(&self, state: &P2pState, _time: redux::Timestamp) -> bool {
        let scheduler = &state.network.scheduler;
        let stream = scheduler
            .identify_state
            .find_identify_stream_state(self.peer_id(), self.stream_id());
        if !matches!(self, Self::Prune { .. })
            && !scheduler
                .connections
                .get(self.addr())
                .is_some_and(|c| c.closed.is_none() && c.peer_id() == Some(self.peer_id()))
        {
            return false;
        }
        self.enabled_for(stream)
    }
}

impl P2pNetworkIdentifyStreamAction {
    fn enabled_for(&self, stream: Option<&P2pNetworkIdentifyStreamState>) -> bool {
        use P2pNetworkIdentifyStreamState as S;
        match self {
            Self::New { .. } => stream.is_none(),
            Self::IncomingData { .. } => matches!(
                stream,
                Some(S::RecvIdentify | S::IncomingPartialData { .. })
            ),
            Self::SendIdentify { .. } => matches!(stream, Some(S::SendIdentify)),
            Self::Close { .. } | Self::RemoteClose { .. } => matches!(
                stream,
                Some(
                    S::RecvIdentify
                        | S::IncomingPartialData { .. }
                        | S::SendIdentify
                        | S::IdentifySent
                        | S::IdentifyReceived { .. }
                )
            ),
            Self::Prune { .. } => matches!(stream, Some(S::Closing | S::Error(_))),
        }
    }
}

impl From<P2pNetworkIdentifyStreamAction> for P2pAction {
    fn from(value: P2pNetworkIdentifyStreamAction) -> Self {
        P2pAction::Network(super::super::P2pNetworkIdentifyAction::Stream(value).into())
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;

    #[test]
    fn identify_ordering_matrix() {
        use P2pNetworkIdentifyStreamState as S;
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
            Some(S::Default),
            Some(S::RecvIdentify),
            Some(S::IncomingPartialData {
                len: 10,
                data: vec![0],
            }),
            Some(S::SendIdentify),
            Some(S::IdentifySent),
            Some(S::Closing),
            Some(S::Error(
                crate::P2pNetworkStreamProtobufError::MessageLength,
            )),
        ];
        let cases = [
            (
                P2pNetworkIdentifyStreamAction::New {
                    addr,
                    peer_id,
                    stream_id,
                    incoming: true,
                },
                vec![0],
            ),
            (
                P2pNetworkIdentifyStreamAction::IncomingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::empty(),
                },
                vec![2, 3],
            ),
            (
                P2pNetworkIdentifyStreamAction::SendIdentify {
                    addr,
                    peer_id,
                    stream_id,
                    addresses: vec![],
                },
                vec![4],
            ),
            (
                P2pNetworkIdentifyStreamAction::Close {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![2, 3, 4, 5],
            ),
            (
                P2pNetworkIdentifyStreamAction::RemoteClose {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![2, 3, 4, 5],
            ),
            (
                P2pNetworkIdentifyStreamAction::Prune {
                    addr,
                    peer_id,
                    stream_id,
                },
                vec![6, 7],
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
