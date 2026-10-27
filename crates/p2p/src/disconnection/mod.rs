mod p2p_disconnection_state;
pub use p2p_disconnection_state::*;

mod p2p_disconnection_actions;
pub use p2p_disconnection_actions::*;

mod p2p_disconnection_reducer;

use serde::{Deserialize, Serialize};

use crate::{
    channels::{rpc::P2pRpcKind, streaming_rpc::P2pStreamingRpcKind, ChannelId},
    connection::RejectionReason,
};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, thiserror::Error)]
pub enum P2pDisconnectionReason {
    #[error("random disconnection in order to free up space so that more peers can connect")]
    FreeUpSpace,
    #[error("message is unexpected for channel {0}")]
    P2pChannelMsgUnexpected(ChannelId),
    #[error("failed to send message to channel: {0}")]
    P2pChannelSendFailed(String),
    #[error("failed to receive message from channel: {0}")]
    P2pChannelReceiveFailed(String),
    #[error("channel {0} is closed")]
    P2pChannelClosed(ChannelId),
    #[error("connection is rejected: {0}")]
    Libp2pIncomingRejected(RejectionReason),
    #[error("transition frontier RPC({0:?}) timeout")]
    TransitionFrontierRpcTimeout(P2pRpcKind),
    #[error("transition frontier streaming RPC({0:?}) timeout")]
    TransitionFrontierStreamingRpcTimeout(P2pStreamingRpcKind),
    #[error("received num accounts rejected")]
    TransitionFrontierSyncLedgerSnarkedNumAccountsRejected,
    #[error("failed to verify snark pool diff")]
    SnarkPoolVerifyError,
    #[error("duplicate connection")]
    DuplicateConnection,
    #[error("timeout")]
    Timeout,
    #[error("rpc protocol not supported")]
    Unsupported,
    #[error("invalid pubsub message")]
    InvalidMessage,
}

impl P2pDisconnectionReason {
    /// Reasons that indicate invalid peer data or a protocol violation.
    pub fn is_misbehaviour(&self) -> bool {
        matches!(
            self,
            Self::P2pChannelMsgUnexpected(_)
                | Self::TransitionFrontierSyncLedgerSnarkedNumAccountsRejected
                | Self::SnarkPoolVerifyError
                | Self::Unsupported
                | Self::InvalidMessage
        ) || matches!(
            self,
            Self::Libp2pIncomingRejected(
                RejectionReason::PeerIdAndPublicKeyMismatch | RejectionReason::TargetPeerIdNotMe
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnection_levels_follow_peer_misbehaviour() {
        assert!(P2pDisconnectionReason::SnarkPoolVerifyError.is_misbehaviour());
        assert!(P2pDisconnectionReason::InvalidMessage.is_misbehaviour());
        assert!(P2pDisconnectionReason::Unsupported.is_misbehaviour());
        assert!(P2pDisconnectionReason::Libp2pIncomingRejected(
            RejectionReason::PeerIdAndPublicKeyMismatch
        )
        .is_misbehaviour());
        assert!(!P2pDisconnectionReason::FreeUpSpace.is_misbehaviour());
        assert!(!P2pDisconnectionReason::DuplicateConnection.is_misbehaviour());
        assert!(!P2pDisconnectionReason::P2pChannelClosed(ChannelId::Rpc).is_misbehaviour());
        assert!(
            !P2pDisconnectionReason::Libp2pIncomingRejected(RejectionReason::PeerCapacityFull)
                .is_misbehaviour()
        );
        assert!(
            !P2pDisconnectionReason::Libp2pIncomingRejected(RejectionReason::AlreadyConnected)
                .is_misbehaviour()
        );
    }
}
