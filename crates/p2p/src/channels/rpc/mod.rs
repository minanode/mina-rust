mod serving_limits;
pub use serving_limits::*;

mod catchup;
pub use catchup::*;

mod p2p_channels_rpc_state;
pub use p2p_channels_rpc_state::*;

mod p2p_channels_rpc_actions;
pub use p2p_channels_rpc_actions::*;

mod p2p_channels_rpc_reducer;

mod completed_snarks_budget;
pub use completed_snarks_budget::*;

mod p2p_channels_rpc_best_tip_budget;
pub use p2p_channels_rpc_best_tip_budget::*;

use std::{sync::Arc, time::Duration};

use binprot_derive::{BinProtRead, BinProtWrite};
use mina_core::{
    block::ArcBlock,
    snark::{Snark, SnarkJobId},
    transaction::{Transaction, TransactionHash},
};
use mina_p2p_messages::{
    list::List,
    rpc_kernel::QueryID,
    v2::{
        LedgerHash, MerkleAddressBinableArgStableV1, MinaBasePendingCoinbaseStableV2,
        MinaBaseStateBodyHashStableV1, MinaLedgerSyncLedgerAnswerStableV2,
        MinaLedgerSyncLedgerQueryStableV1, MinaStateProtocolStateValueStableV2, StateHash,
        TransactionSnarkScanStateStableV2,
    },
};
use serde::{Deserialize, Serialize};

use crate::{connection::outgoing::P2pConnectionOutgoingInitOpts, P2pTimeouts};

pub type P2pRpcId = QueryID;

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub enum RpcChannelMsg {
    Request(P2pRpcId, P2pRpcRequest),
    Response(P2pRpcId, Option<P2pRpcResponse>),
}

impl RpcChannelMsg {
    pub fn request_id(&self) -> P2pRpcId {
        match self {
            Self::Request(id, _) => *id,
            Self::Response(id, _) => *id,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq, Clone)]
pub enum P2pRpcKind {
    BestTipWithProof,
    LedgerQuery,
    StagedLedgerAuxAndPendingCoinbasesAtBlock,
    Block,
    Snark,
    Transaction,
    InitialPeers,
    Catchup,
    CompletedSnarks,
}

impl P2pRpcKind {
    pub fn timeout(self, config: &P2pTimeouts) -> Option<Duration> {
        match self {
            Self::BestTipWithProof => config.best_tip_with_proof,
            Self::LedgerQuery => config.ledger_query,
            Self::StagedLedgerAuxAndPendingCoinbasesAtBlock => {
                config.staged_ledger_aux_and_pending_coinbases_at_block
            }
            Self::Block => config.block,
            Self::Snark | Self::CompletedSnarks => config.snark,
            Self::Transaction => config.transaction,
            Self::InitialPeers => config.initial_peers,
            Self::Catchup => None,
        }
    }

    pub fn supported_by_libp2p(self) -> bool {
        match self {
            Self::BestTipWithProof => true,
            Self::LedgerQuery => true,
            Self::StagedLedgerAuxAndPendingCoinbasesAtBlock => true,
            Self::Block => true,
            Self::Snark => false,
            Self::Transaction => false,
            Self::InitialPeers => true,
            Self::Catchup | Self::CompletedSnarks => true,
        }
    }
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
pub enum P2pRpcRequest {
    #[default]
    BestTipWithProof,
    LedgerQuery(LedgerHash, MinaLedgerSyncLedgerQueryStableV1),
    StagedLedgerAuxAndPendingCoinbasesAtBlock(StateHash),
    Block(StateHash),
    Snark(SnarkJobId),
    Transaction(TransactionHash),
    InitialPeers,
    Catchup(CatchupRequest),
    CompletedSnarks,
}

impl P2pRpcRequest {
    pub fn kind(&self) -> P2pRpcKind {
        match self {
            Self::BestTipWithProof => P2pRpcKind::BestTipWithProof,
            Self::LedgerQuery(..) => P2pRpcKind::LedgerQuery,
            Self::StagedLedgerAuxAndPendingCoinbasesAtBlock(_) => {
                P2pRpcKind::StagedLedgerAuxAndPendingCoinbasesAtBlock
            }
            Self::Block(_) => P2pRpcKind::Block,
            Self::Snark(_) => P2pRpcKind::Snark,
            Self::Transaction(_) => P2pRpcKind::Transaction,
            Self::InitialPeers => P2pRpcKind::InitialPeers,
            Self::Catchup(_) => P2pRpcKind::Catchup,
            Self::CompletedSnarks => P2pRpcKind::CompletedSnarks,
        }
    }
}

fn addr_to_str(
    MerkleAddressBinableArgStableV1(mina_p2p_messages::number::Number(length), byte_string): &MerkleAddressBinableArgStableV1,
) -> String {
    let addr = byte_string
        .as_ref()
        .iter()
        .copied()
        .flat_map(|byte| {
            (0..8)
                .map(move |b| byte & (1 << (7 - b)) != 0)
                .map(|b| if b { '1' } else { '0' })
        })
        .take(*length as usize)
        .collect::<String>();

    format!("depth: {length}, addr: {addr}")
}

impl std::fmt::Display for P2pRpcRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.kind())?;
        match self {
            Self::BestTipWithProof => Ok(()),
            Self::LedgerQuery(ledger_hash, query) => {
                match query {
                    MinaLedgerSyncLedgerQueryStableV1::NumAccounts => write!(f, ", NumAccounts, ")?,
                    MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(addr, _) => {
                        write!(f, ", ChildHashes, {}, ", addr_to_str(addr))?
                    }
                    MinaLedgerSyncLedgerQueryStableV1::WhatContents(addr) => {
                        write!(f, ", ChildContents, {}, ", addr_to_str(addr))?
                    }
                }
                write!(f, "ledger: {ledger_hash}")
            }
            Self::StagedLedgerAuxAndPendingCoinbasesAtBlock(block_hash)
            | Self::Block(block_hash) => {
                write!(f, ", {block_hash}")
            }
            Self::Snark(job_id) => {
                write!(f, ", {job_id}")
            }
            Self::Transaction(hash) => {
                write!(f, ", {hash}")
            }
            Self::InitialPeers | Self::CompletedSnarks => Ok(()),
            Self::Catchup(request) => write!(f, "{request:?}"),
        }
    }
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub struct BestTipWithProof {
    pub best_tip: ArcBlock,
    pub proof: (List<MinaBaseStateBodyHashStableV1>, ArcBlock),
}

/// Pieces required to reconstruct staged ledger from snarked ledger.
#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub struct StagedLedgerAuxAndPendingCoinbases {
    pub scan_state: TransactionSnarkScanStateStableV2,
    pub staged_ledger_hash: LedgerHash,
    pub pending_coinbase: MinaBasePendingCoinbaseStableV2,
    pub needed_blocks: List<MinaStateProtocolStateValueStableV2>,
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub enum P2pRpcResponse {
    BestTipWithProof(BestTipWithProof),
    LedgerQuery(MinaLedgerSyncLedgerAnswerStableV2),
    StagedLedgerAuxAndPendingCoinbasesAtBlock(Arc<StagedLedgerAuxAndPendingCoinbases>),
    Block(ArcBlock),
    Snark(Snark),
    Transaction(Transaction),
    InitialPeers(List<P2pConnectionOutgoingInitOpts>),
    Catchup(Box<CatchupResponse>),
    CompletedSnarks(mina_p2p_messages::rpc::CompletedSnarks),
    /// Internal serving error; WebRTC encodes this as its existing None response.
    LedgerQueryError(LedgerHash, mina_p2p_messages::string::CharString),
}

impl P2pRpcResponse {
    pub fn kind(&self) -> P2pRpcKind {
        match self {
            Self::BestTipWithProof(_) => P2pRpcKind::BestTipWithProof,
            Self::LedgerQuery(_) | Self::LedgerQueryError(..) => P2pRpcKind::LedgerQuery,
            Self::StagedLedgerAuxAndPendingCoinbasesAtBlock(_) => {
                P2pRpcKind::StagedLedgerAuxAndPendingCoinbasesAtBlock
            }
            Self::Block(_) => P2pRpcKind::Block,
            Self::Snark(_) => P2pRpcKind::Snark,
            Self::Transaction(_) => P2pRpcKind::Transaction,
            Self::CompletedSnarks(_) => P2pRpcKind::CompletedSnarks,
            Self::InitialPeers(_) => P2pRpcKind::InitialPeers,
            Self::Catchup(_) => P2pRpcKind::Catchup,
        }
    }
}

#[cfg(feature = "p2p-libp2p")]
mod libp2p {
    use super::*;
    use crate::Data;
    use mina_p2p_messages::{
        rpc,
        rpc_kernel::{
            NeedsLength, QueryHeader, QueryPayload, ResponseHeader, ResponsePayload, RpcMethod,
            RpcResult,
        },
    };

    pub fn unavailable_response_into_libp2p(
        request: &P2pRpcRequest,
        id: P2pRpcId,
    ) -> Option<(ResponseHeader, Data)> {
        let bytes = match request {
            P2pRpcRequest::BestTipWithProof => response_bytes::<rpc::GetBestTipV2Response>(None),
            P2pRpcRequest::Block(_) => {
                response_bytes::<<rpc::GetTransitionChainV2 as RpcMethod>::Response>(None)
            }
            P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(_) => {
                response_bytes::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>(None)
            }
            P2pRpcRequest::CompletedSnarks => {
                response_bytes::<<rpc::GetCompletedSnarksV1 as RpcMethod>::Response>(None)
            }
            P2pRpcRequest::LedgerQuery(..) => {
                response_bytes::<<rpc::AnswerSyncLedgerQueryV2 as RpcMethod>::Response>(RpcResult(
                    Err(mina_p2p_messages::core::Error::new("Ledger unavailable")),
                ))
            }
            P2pRpcRequest::InitialPeers => response_bytes::<
                <rpc::GetSomeInitialPeersV1ForV2 as RpcMethod>::Response,
            >(List::new()),
            P2pRpcRequest::Catchup(CatchupRequest::Ancestry(_)) => {
                response_bytes::<rpc::GetAncestryV2Response>(None)
            }
            P2pRpcRequest::Catchup(CatchupRequest::TransitionChain(_)) => {
                response_bytes::<<rpc::GetTransitionChainV2 as RpcMethod>::Response>(None)
            }
            P2pRpcRequest::Catchup(CatchupRequest::TransitionChainProof(_)) => {
                response_bytes::<rpc::GetTransitionChainProofV1ForV2Response>(None)
            }
            P2pRpcRequest::Catchup(CatchupRequest::TransitionKnowledge) => {
                response_bytes::<<rpc::GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response>(
                    List::new(),
                )
            }
            _ => return None,
        };
        Some((ResponseHeader { id }, bytes.into()))
    }

    pub fn internal_response_into_libp2p(
        response: P2pRpcResponse,
        id: P2pRpcId,
    ) -> Option<(ResponseHeader, Data)> {
        use binprot::BinProtWrite;

        match response {
            P2pRpcResponse::Catchup(r) => Some((ResponseHeader { id }, r.into_bytes().into())),
            P2pRpcResponse::BestTipWithProof(r) => {
                type Method = rpc::GetBestTipV2;
                type Payload = ResponsePayload<<Method as RpcMethod>::Response>;

                let BestTipWithProof {
                    best_tip,
                    proof: (middle, block),
                } = r;

                let r = RpcResult(Ok(NeedsLength(Some(rpc::ProofCarryingDataStableV1 {
                    data: best_tip.as_ref().clone(),
                    proof: (middle, block.as_ref().clone()),
                }))));

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&r, &mut v)
                    .inspect_err(|error| {
                        mina_core::log::inner::warn!("RPC response unavailable: {error}")
                    })
                    .ok()?;
                Some((ResponseHeader { id: id as _ }, v.into()))
            }
            P2pRpcResponse::LedgerQueryError(hash, error) => {
                // OCaml rpcs.ml:426-432 wraps the responder error with the
                // ledger hash sexp (the field bigint, in hexadecimal).
                let hex = hash
                    .0
                    .to_bytes()
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02X}"))
                    .collect::<String>();
                let error = format!(
                    "Refusing to answer sync ledger query for ledger_hash: 0x{hex}. Error: {error}"
                );
                let response = RpcResult::<(), _>(Err(mina_p2p_messages::core::Error::new(&error)));
                Some((ResponseHeader { id }, response_bytes(response).into()))
            }
            P2pRpcResponse::LedgerQuery(answer) => {
                type Method = rpc::AnswerSyncLedgerQueryV2;
                type Payload = ResponsePayload<<Method as RpcMethod>::Response>;

                let r = RpcResult(Ok(NeedsLength(RpcResult(Ok(answer)))));

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&r, &mut v)
                    .inspect_err(|error| {
                        mina_core::log::inner::warn!("RPC response unavailable: {error}")
                    })
                    .ok()?;
                Some((ResponseHeader { id: id as _ }, v.into()))
            }
            P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(staged_ledger_info) => {
                type Method = rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2;
                type Payload = ResponsePayload<<Method as RpcMethod>::Response>;

                let StagedLedgerAuxAndPendingCoinbases {
                    scan_state,
                    staged_ledger_hash,
                    pending_coinbase,
                    needed_blocks,
                } = staged_ledger_info.as_ref().clone();

                let hash = staged_ledger_hash.inner().0.clone();

                let r = RpcResult(Ok(NeedsLength(Some((
                    scan_state,
                    hash,
                    pending_coinbase,
                    needed_blocks,
                )))));

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&r, &mut v)
                    .inspect_err(|error| {
                        mina_core::log::inner::warn!("RPC response unavailable: {error}")
                    })
                    .ok()?;
                Some((ResponseHeader { id: id as _ }, v.into()))
            }
            P2pRpcResponse::Block(block) => {
                type Method = rpc::GetTransitionChainV2;
                type Payload = ResponsePayload<<Method as RpcMethod>::Response>;

                let r = RpcResult(Ok(NeedsLength(Some(List::one(block.as_ref().clone())))));

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&r, &mut v)
                    .inspect_err(|error| {
                        mina_core::log::inner::warn!("RPC response unavailable: {error}")
                    })
                    .ok()?;
                Some((ResponseHeader { id: id as _ }, v.into()))
            }
            P2pRpcResponse::CompletedSnarks(works) => {
                Some((ResponseHeader { id }, response_bytes(Some(works)).into()))
            }
            P2pRpcResponse::Snark(_) => {
                // should use gossipsub to broadcast
                None
            }
            P2pRpcResponse::Transaction(_) => {
                // should use gossipsub to broadcast
                None
            }
            P2pRpcResponse::InitialPeers(peers) => {
                type Method = rpc::GetSomeInitialPeersV1ForV2;
                type Payload = ResponsePayload<<Method as RpcMethod>::Response>;

                let r = peers
                    .into_iter()
                    .filter_map(|peer| peer.try_into_mina_rpc())
                    .collect();
                let r = RpcResult(Ok(NeedsLength(r)));

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&r, &mut v)
                    .inspect_err(|error| {
                        mina_core::log::inner::warn!("RPC response unavailable: {error}")
                    })
                    .ok()?;
                Some((ResponseHeader { id: id as _ }, v.into()))
            }
        }
    }

    pub fn internal_request_into_libp2p(
        request: P2pRpcRequest,
        id: P2pRpcId,
    ) -> Option<(QueryHeader, Data)> {
        use binprot::BinProtWrite;

        match request {
            P2pRpcRequest::CompletedSnarks => {
                let mut data = Vec::new();
                NeedsLength(()).binprot_write(&mut data).unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: rpc::GetCompletedSnarksV1::NAME.into(),
                        version: rpc::GetCompletedSnarksV1::VERSION,
                        id,
                    },
                    data.into(),
                ))
            }
            P2pRpcRequest::Catchup(_) => None, // inbound only
            P2pRpcRequest::BestTipWithProof => {
                type Method = rpc::GetBestTipV2;
                type Payload = QueryPayload<<Method as RpcMethod>::Query>;

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&NeedsLength(()), &mut v)
                    .unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: Method::NAME.into(),
                        version: Method::VERSION,
                        id: id as _,
                    },
                    v.into(),
                ))
            }
            P2pRpcRequest::LedgerQuery(hash, q) => {
                type Method = rpc::AnswerSyncLedgerQueryV2;
                type Payload = QueryPayload<<Method as RpcMethod>::Query>;

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&NeedsLength((hash.0.clone(), q)), &mut v)
                    .unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: Method::NAME.into(),
                        version: Method::VERSION,
                        id: id as _,
                    },
                    v.into(),
                ))
            }
            P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(hash) => {
                type Method = rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2;
                type Payload = QueryPayload<<Method as RpcMethod>::Query>;

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&NeedsLength(hash.0.clone()), &mut v)
                    .unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: Method::NAME.into(),
                        version: Method::VERSION,
                        id: id as _,
                    },
                    v.into(),
                ))
            }
            P2pRpcRequest::Block(hash) => {
                type Method = rpc::GetTransitionChainV2;
                type Payload = QueryPayload<<Method as RpcMethod>::Query>;

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(
                    &NeedsLength(List::one(hash.0.clone())),
                    &mut v,
                )
                .unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: Method::NAME.into(),
                        version: Method::VERSION,
                        id: id as _,
                    },
                    v.into(),
                ))
            }
            P2pRpcRequest::Snark(hash) => {
                let _ = hash;
                // libp2p cannot fulfill this request
                None
            }
            P2pRpcRequest::Transaction(hash) => {
                let _ = hash;
                // libp2p cannot fulfill this request
                None
            }
            P2pRpcRequest::InitialPeers => {
                type Method = rpc::GetSomeInitialPeersV1ForV2;
                type Payload = QueryPayload<<Method as RpcMethod>::Query>;

                let mut v = vec![];
                <Payload as BinProtWrite>::binprot_write(&NeedsLength(()), &mut v)
                    .unwrap_or_default();
                Some((
                    QueryHeader {
                        tag: Method::NAME.into(),
                        version: Method::VERSION,
                        id: id as _,
                    },
                    v.into(),
                ))
            }
        }
    }
}
