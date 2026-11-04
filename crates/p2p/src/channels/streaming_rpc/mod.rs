mod rpcs;
pub use rpcs::*;

mod p2p_channels_streaming_rpc_state;
pub use p2p_channels_streaming_rpc_state::*;

mod p2p_channels_streaming_rpc_actions;
pub use p2p_channels_streaming_rpc_actions::*;

mod p2p_channels_streaming_rpc_reducer;

use binprot_derive::{BinProtRead, BinProtWrite};
use serde::{Deserialize, Serialize};

pub type P2pStreamingRpcId = u64;

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub enum StreamingRpcChannelMsg {
    /// Send the next part.
    Next(P2pStreamingRpcId),
    Request(P2pStreamingRpcId, P2pStreamingRpcRequest),
    Response(P2pStreamingRpcId, Option<P2pStreamingRpcResponse>),
    /// Send no more parts: the requester has them from another peer. Only
    /// to a peer that advertised `staged_ledger_from_last`; an older one
    /// can't decode it.
    Cancel(P2pStreamingRpcId),
}

impl StreamingRpcChannelMsg {
    pub fn request_id(&self) -> P2pStreamingRpcId {
        match self {
            Self::Next(id) => *id,
            Self::Request(id, _) => *id,
            Self::Response(id, _) => *id,
            Self::Cancel(id) => *id,
        }
    }
}
