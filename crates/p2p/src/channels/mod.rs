pub mod best_tip;
pub mod rpc;
pub mod signaling;
pub mod snark;
pub mod snark_job_commitment;
pub mod streaming_rpc;
pub mod transaction;

mod p2p_channels_state;
pub use p2p_channels_state::*;

mod p2p_channels_actions;
pub use p2p_channels_actions::*;

mod p2p_channels_reducer;

mod p2p_channels_service;
pub use p2p_channels_service::*;

mod p2p_channels_effectful_effects;

mod request_timeout;
pub use request_timeout::*;

use binprot::{BinProtRead, BinProtWrite};
use binprot_derive::{BinProtRead, BinProtWrite};
use derive_more::From;
use serde::{Deserialize, Serialize};
use signaling::{discovery::SignalingDiscoveryChannelMsg, exchange::SignalingExchangeChannelMsg};
use strum_macros::EnumIter;

use self::{
    best_tip::BestTipPropagationChannelMsg, rpc::RpcChannelMsg, snark::SnarkPropagationChannelMsg,
    snark_job_commitment::SnarkJobCommitmentPropagationChannelMsg,
    streaming_rpc::StreamingRpcChannelMsg, transaction::TransactionPropagationChannelMsg,
};

#[derive(Serialize, Deserialize, EnumIter, Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum ChannelId {
    SignalingDiscovery = 1,
    SignalingExchange = 2,
    BestTipPropagation = 3,
    TransactionPropagation = 4,
    SnarkPropagation = 5,
    SnarkJobCommitmentPropagation = 6,
    Rpc = 7,
    StreamingRpc = 8,
}

/// The largest message we take on the streaming RPC channel, 128 MiB.
///
/// The largest messages on this channel are staged ledger parts. A part is one
/// scan-state tree, or the scan state's base, whose last emitted proof comes
/// with the transactions of the tree that made it. On mainnet and devnet a
/// tree has 128 base jobs and 127 merge jobs. A base job keeps its transaction
/// in full, proofs included, and two sparse ledger witnesses until the tree is
/// emitted.
///
/// The devnet scan state at block 590216 has a tree of 37,273,328 bytes, with
/// 103 zkApp commands of 15 proofs each. Each of those base jobs is 345,610
/// bytes: 240,500 for the command and 104,146 for its witnesses. A full tree
/// of 128 such jobs is 44.2 MB. With the old 16 MiB limit, no web node could
/// sync while such a tree was in the root's scan state.
///
/// 128 MiB is three times that full tree. It is a practical cap, not a
/// protocol bound. The protocol counts the field elements in a zkApp
/// command's events and actions, not the events themselves, so a command can
/// carry any number of empty events, and nothing bounds its serialized size.
/// A web node cannot fetch a staged ledger with a tree over 128 MiB.
///
/// A peer has to send the bytes before we hold them. The receive buffer grows
/// with the bytes received and never past the declared length. The RPC channel
/// already takes messages of 256 MiB.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_command.ml#L1344-L1423>
pub const STREAMING_RPC_MAX_MSG_SIZE: usize = 128 * 1024 * 1024;

impl ChannelId {
    /// Data channels for direct peers, without peer-to-peer signaling relay.
    pub fn without_signaling() -> impl Iterator<Item = Self> {
        Self::iter_all()
            .filter(|id| !matches!(id, Self::SignalingDiscovery | Self::SignalingExchange))
    }

    #[inline(always)]
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    #[inline(always)]
    pub fn to_u16(self) -> u16 {
        self as u16
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SignalingDiscovery => "signaling/discovery",
            Self::SignalingExchange => "signaling/exchange",
            Self::BestTipPropagation => "best_tip/propagation",
            Self::TransactionPropagation => "transaction/propagation",
            Self::SnarkPropagation => "snark/propagation",
            Self::SnarkJobCommitmentPropagation => "snark_job_commitment/propagation",
            Self::Rpc => "rpc",
            Self::StreamingRpc => "rpc/streaming",
        }
    }

    pub fn supported_by_libp2p(self) -> bool {
        match self {
            Self::SignalingDiscovery => false,
            Self::SignalingExchange => false,
            Self::BestTipPropagation => true,
            Self::TransactionPropagation => true,
            Self::SnarkPropagation => true,
            Self::SnarkJobCommitmentPropagation => false,
            Self::Rpc => true,
            Self::StreamingRpc => false,
        }
    }

    pub fn max_msg_size(self) -> usize {
        match self {
            // TODO(binier): measure signaling message sizes
            Self::SignalingDiscovery => 16 * 1024, // 16KB
            Self::SignalingExchange => 16 * 1024,  // 16KB
            // TODO(binier): reduce this value once we change message for best tip
            // propagation to just propagating consensus state with block hash.
            Self::BestTipPropagation => 32 * 1024 * 1024, // 32MB
            Self::TransactionPropagation => 1024,         // 1KB - just transaction info.
            Self::SnarkPropagation => 1024,               // 1KB - just snark info.
            Self::SnarkJobCommitmentPropagation => 2 * 1024, // 2KB,
            Self::Rpc => 256 * 1024 * 1024,               // 256MB,
            Self::StreamingRpc => STREAMING_RPC_MAX_MSG_SIZE,
        }
    }

    pub fn iter_all() -> impl Iterator<Item = ChannelId> {
        <Self as strum::IntoEnumIterator>::iter()
    }

    pub fn for_libp2p() -> impl Iterator<Item = ChannelId> {
        Self::iter_all().filter(|chan| chan.supported_by_libp2p())
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_u8())
    }
}

#[derive(Serialize, Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
pub struct MsgId(u64);

impl MsgId {
    pub fn first() -> Self {
        Self(1)
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, From, Debug, Clone)]
pub enum ChannelMsg {
    SignalingDiscovery(SignalingDiscoveryChannelMsg),
    SignalingExchange(SignalingExchangeChannelMsg),
    BestTipPropagation(BestTipPropagationChannelMsg),
    TransactionPropagation(TransactionPropagationChannelMsg),
    SnarkPropagation(SnarkPropagationChannelMsg),
    SnarkJobCommitmentPropagation(SnarkJobCommitmentPropagationChannelMsg),
    Rpc(RpcChannelMsg),
    StreamingRpc(StreamingRpcChannelMsg),
}

impl ChannelMsg {
    pub fn channel_id(&self) -> ChannelId {
        match self {
            Self::SignalingDiscovery(_) => ChannelId::SignalingDiscovery,
            Self::SignalingExchange(_) => ChannelId::SignalingExchange,
            Self::BestTipPropagation(_) => ChannelId::BestTipPropagation,
            Self::TransactionPropagation(_) => ChannelId::TransactionPropagation,
            Self::SnarkPropagation(_) => ChannelId::SnarkPropagation,
            Self::SnarkJobCommitmentPropagation(_) => ChannelId::SnarkJobCommitmentPropagation,
            Self::Rpc(_) => ChannelId::Rpc,
            Self::StreamingRpc(_) => ChannelId::StreamingRpc,
        }
    }

    pub fn encode<W>(&self, w: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        match self {
            Self::SignalingDiscovery(v) => v.binprot_write(w),
            Self::SignalingExchange(v) => v.binprot_write(w),
            Self::BestTipPropagation(v) => v.binprot_write(w),
            Self::TransactionPropagation(v) => v.binprot_write(w),
            Self::SnarkPropagation(v) => v.binprot_write(w),
            Self::SnarkJobCommitmentPropagation(v) => v.binprot_write(w),
            Self::Rpc(v) => v.binprot_write(w),
            Self::StreamingRpc(v) => v.binprot_write(w),
        }
    }

    pub fn decode<R>(r: &mut R, id: ChannelId) -> Result<Self, binprot::Error>
    where
        Self: Sized,
        R: std::io::Read + ?Sized,
    {
        match id {
            ChannelId::SignalingDiscovery => {
                SignalingDiscoveryChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::SignalingExchange => {
                SignalingExchangeChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::BestTipPropagation => {
                BestTipPropagationChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::TransactionPropagation => {
                TransactionPropagationChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::SnarkPropagation => {
                SnarkPropagationChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::SnarkJobCommitmentPropagation => {
                SnarkJobCommitmentPropagationChannelMsg::binprot_read(r).map(|v| v.into())
            }
            ChannelId::Rpc => RpcChannelMsg::binprot_read(r).map(|v| v.into()),
            ChannelId::StreamingRpc => StreamingRpcChannelMsg::binprot_read(r).map(|v| v.into()),
        }
    }
}

impl crate::P2pState {
    /// Initializes enabled channels.
    pub fn channels_init<Action, State>(
        &self,
        dispatcher: &mut redux::Dispatcher<Action, State>,
        peer_id: crate::PeerId,
    ) where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        // Dispatches can be done without a loop, but inside we do
        // exhaustive matching so that we don't miss any channels.
        for id in self.config.enabled_channels.iter().copied() {
            if self.peers.get(&peer_id).is_some_and(|p| p.is_libp2p()) && !id.supported_by_libp2p()
            {
                continue;
            }
            // A sharing browser opens the signalling channels to its seeds only.
            if matches!(
                id,
                ChannelId::SignalingDiscovery | ChannelId::SignalingExchange
            ) && !self.signaling_allowed_with(&peer_id)
            {
                continue;
            }
            match id {
                ChannelId::SignalingDiscovery => {
                    dispatcher.push(
                        signaling::discovery::P2pChannelsSignalingDiscoveryAction::Init { peer_id },
                    );
                }
                ChannelId::SignalingExchange => {
                    dispatcher.push(
                        signaling::exchange::P2pChannelsSignalingExchangeAction::Init { peer_id },
                    );
                }
                ChannelId::BestTipPropagation => {
                    dispatcher.push(best_tip::P2pChannelsBestTipAction::Init { peer_id });
                }
                ChannelId::TransactionPropagation => {
                    dispatcher.push(transaction::P2pChannelsTransactionAction::Init { peer_id });
                }
                ChannelId::SnarkPropagation => {
                    dispatcher.push(snark::P2pChannelsSnarkAction::Init { peer_id });
                }
                ChannelId::SnarkJobCommitmentPropagation => {
                    dispatcher.push(
                        snark_job_commitment::P2pChannelsSnarkJobCommitmentAction::Init { peer_id },
                    );
                }
                ChannelId::Rpc => {
                    dispatcher.push(rpc::P2pChannelsRpcAction::Init { peer_id });
                }
                ChannelId::StreamingRpc => {
                    dispatcher.push(streaming_rpc::P2pChannelsStreamingRpcAction::Init { peer_id });
                }
            }
        }
    }
}
