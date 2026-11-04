pub mod staged_ledger_parts;
use staged_ledger_parts::{
    StagedLedgerPartsFromLastProgress, StagedLedgerPartsReceiveProgress, StagedLedgerPartsResponse,
    StagedLedgerPartsResponseFull, StagedLedgerPartsSendProgress,
};

use std::time::Duration;

use binprot_derive::{BinProtRead, BinProtWrite};
use derive_more::From;
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use crate::P2pTimeouts;

#[derive(Serialize, Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq, Clone)]
pub enum P2pStreamingRpcKind {
    StagedLedgerParts,
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum P2pStreamingRpcRequest {
    StagedLedgerParts(v2::StateHash),
    /// The same parts in another order, so that two peers can send one
    /// staged ledger from its two ends at once (STAGED-SPLIT): the scan
    /// state's base, its trees last first, the previous incomplete zkapp
    /// updates, then the base. Only to a peer that advertised
    /// `staged_ledger_from_last`; an older one can't decode it.
    StagedLedgerPartsFromLast(v2::StateHash),
}

#[derive(Serialize, Deserialize, From, Debug, Clone)]
pub enum P2pStreamingRpcResponseFull {
    StagedLedgerParts(StagedLedgerPartsResponseFull),
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, From, Debug, Clone)]
pub enum P2pStreamingRpcResponse {
    StagedLedgerParts(StagedLedgerPartsResponse),
}

#[derive(Serialize, Deserialize, From, Debug, Clone)]
pub enum P2pStreamingRpcSendProgress {
    StagedLedgerParts(StagedLedgerPartsSendProgress),
}

#[derive(Serialize, Deserialize, From, Debug, Clone)]
pub enum P2pStreamingRpcReceiveProgress {
    StagedLedgerParts(StagedLedgerPartsReceiveProgress),
    StagedLedgerPartsFromLast(StagedLedgerPartsFromLastProgress),
}

impl P2pStreamingRpcKind {
    pub fn timeout(self, _config: &P2pTimeouts) -> Option<Duration> {
        match self {
            // TODO(binier): use config
            Self::StagedLedgerParts => Some(Duration::from_secs(30)),
        }
    }
}

impl P2pStreamingRpcRequest {
    pub fn kind(&self) -> P2pStreamingRpcKind {
        match self {
            Self::StagedLedgerParts(_) | Self::StagedLedgerPartsFromLast(_) => {
                P2pStreamingRpcKind::StagedLedgerParts
            }
        }
    }

    /// The block whose staged ledger is asked for.
    pub fn staged_ledger_block_hash(&self) -> &v2::StateHash {
        match self {
            Self::StagedLedgerParts(hash) | Self::StagedLedgerPartsFromLast(hash) => hash,
        }
    }

    /// Whether the parts come from the last ([`Self::StagedLedgerPartsFromLast`]).
    pub fn is_from_last(&self) -> bool {
        matches!(self, Self::StagedLedgerPartsFromLast(_))
    }
}

impl Default for P2pStreamingRpcRequest {
    fn default() -> Self {
        Self::StagedLedgerParts(v2::StateHash::zero())
    }
}

impl std::fmt::Display for P2pStreamingRpcRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.kind())?;
        match self {
            Self::StagedLedgerParts(block_hash) => write!(f, ", {block_hash}"),
            Self::StagedLedgerPartsFromLast(block_hash) => write!(f, ", from last, {block_hash}"),
        }
    }
}

impl P2pStreamingRpcResponseFull {
    pub fn kind(&self) -> P2pStreamingRpcKind {
        match self {
            Self::StagedLedgerParts(_) => P2pStreamingRpcKind::StagedLedgerParts,
        }
    }
}

impl P2pStreamingRpcResponse {
    /// Whether the part is within what a valid response can hold, before
    /// it is kept: a scan state's tree count is the sender's, and the parts
    /// are gathered until that many trees have come.
    pub fn is_within_bounds(&self) -> bool {
        match self {
            Self::StagedLedgerParts(StagedLedgerPartsResponse::ScanStateBase(base)) => {
                base.trees_within_bound()
            }
            Self::StagedLedgerParts(_) => true,
        }
    }

    pub fn kind(&self) -> P2pStreamingRpcKind {
        match self {
            Self::StagedLedgerParts(_) => P2pStreamingRpcKind::StagedLedgerParts,
        }
    }
}

impl P2pStreamingRpcSendProgress {
    pub fn kind(&self) -> P2pStreamingRpcKind {
        match self {
            Self::StagedLedgerParts(_) => P2pStreamingRpcKind::StagedLedgerParts,
        }
    }

    pub fn external_data_todo(&self) -> bool {
        match self {
            Self::StagedLedgerParts(v) => {
                matches!(v, StagedLedgerPartsSendProgress::LedgerGetIdle { .. })
            }
        }
    }

    pub fn external_data_pending(&self) -> bool {
        match self {
            Self::StagedLedgerParts(v) => {
                matches!(v, StagedLedgerPartsSendProgress::LedgerGetPending { .. })
            }
        }
    }

    pub fn next_msg(&self) -> Option<P2pStreamingRpcResponse> {
        match self {
            Self::StagedLedgerParts(v) => v.next_msg().map(Into::into),
        }
    }

    pub fn is_done(&self) -> bool {
        match self {
            Self::StagedLedgerParts(s) => {
                matches!(s, StagedLedgerPartsSendProgress::Success { .. })
            }
        }
    }
}

impl P2pStreamingRpcReceiveProgress {
    pub fn kind(&self) -> P2pStreamingRpcKind {
        match self {
            Self::StagedLedgerParts(_) | Self::StagedLedgerPartsFromLast(_) => {
                P2pStreamingRpcKind::StagedLedgerParts
            }
        }
    }

    pub fn is_done(&self) -> bool {
        match self {
            Self::StagedLedgerParts(s) => {
                matches!(s, StagedLedgerPartsReceiveProgress::Success { .. })
            }
            Self::StagedLedgerPartsFromLast(s) => s.data.is_some(),
        }
    }

    pub fn last_updated(&self) -> redux::Timestamp {
        match self {
            Self::StagedLedgerParts(s) => s.last_updated(),
            Self::StagedLedgerPartsFromLast(s) => s.time,
        }
    }

    /// Whether `resp` is the part this stream expects next. A part out of
    /// order from a stream from the last is unexpected, as from any peer.
    pub fn accepts(&self, resp: &P2pStreamingRpcResponse) -> bool {
        match (self, resp) {
            (Self::StagedLedgerParts(_), P2pStreamingRpcResponse::StagedLedgerParts(_)) => true,
            (
                Self::StagedLedgerPartsFromLast(progress),
                P2pStreamingRpcResponse::StagedLedgerParts(resp),
            ) => progress.accepts(resp),
        }
    }

    pub fn update(&mut self, time: redux::Timestamp, resp: P2pStreamingRpcResponse) -> bool {
        match (self, resp) {
            (
                Self::StagedLedgerParts(progress),
                P2pStreamingRpcResponse::StagedLedgerParts(resp),
            ) => progress.update(time, resp),
            (
                Self::StagedLedgerPartsFromLast(progress),
                P2pStreamingRpcResponse::StagedLedgerParts(resp),
            ) => progress.update(time, resp),
        }
    }

    pub fn is_part_pending(&self) -> bool {
        match self {
            Self::StagedLedgerParts(progress) => progress.is_part_pending(),
            Self::StagedLedgerPartsFromLast(progress) => progress.is_part_pending(),
        }
    }

    pub fn set_next_pending(&mut self, time: redux::Timestamp) -> bool {
        match self {
            Self::StagedLedgerParts(progress) => progress.set_next_pending(time),
            Self::StagedLedgerPartsFromLast(progress) => progress.set_next_pending(time),
        }
    }
}
