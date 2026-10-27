use std::{
    collections::{BTreeMap, VecDeque},
    str,
    time::Duration,
};

use binprot::BinProtWrite;
use serde::{Deserialize, Serialize};

use mina_p2p_messages::{
    rpc_kernel::{MessageHeader, QueryHeader, ResponseHeader},
    string::CharString,
    versioned::Ver,
};

use crate::{channels::rpc::P2pRpcId, Data};

use super::super::*;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
// OCaml's incoming Connection.create uses async_rpc_kernel.v0.14.0 defaults.
// OCaml: <https://github.com/janestreet/async_rpc_kernel/blob/v0.14/src/connection.ml#L38-L46>
const INCOMING_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pNetworkRpcState {
    pub addr: ConnectionAddr,
    pub stream_id: StreamId,
    pub last_id: P2pRpcId,
    #[ignore_malloc_size_of = "primitive"]
    pub last_heartbeat_sent: Option<redux::Timestamp>,
    #[serde(default)]
    #[ignore_malloc_size_of = "primitive"]
    pub last_received: Option<redux::Timestamp>,
    pub pending: Option<QueryHeader>,
    #[serde(default)]
    #[ignore_malloc_size_of = "TODO"]
    pub pending_incoming: BTreeMap<P2pRpcId, PendingIncomingQuery>,
    #[serde_as(as = "Vec<(_, _)>")]
    #[ignore_malloc_size_of = "TODO(vlad)"]
    pub total_stats: BTreeMap<(CharString, Ver), usize>,
    pub is_incoming: bool,
    pub buffer: Vec<u8>,
    pub incoming: VecDeque<RpcMessage>,
    pub error: Option<P2pNetworkRpcError>,
}

/// A reservation exists only while a channel handler owns this query. Removing
/// the stream also removes every reservation it owns.
#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct PendingIncomingQuery {
    #[ignore_malloc_size_of = "TODO(vlad)"]
    pub header: QueryHeader,
    #[ignore_malloc_size_of = "primitive"]
    pub started_at: redux::Timestamp,
    pub reserved_bytes: usize,
}

impl P2pNetworkRpcState {
    pub fn new(addr: ConnectionAddr, stream_id: StreamId) -> Self {
        P2pNetworkRpcState {
            addr,
            stream_id,
            last_id: 0,
            last_heartbeat_sent: None,
            last_received: None,
            pending: None,
            pending_incoming: BTreeMap::new(),
            total_stats: BTreeMap::default(),
            is_incoming: false,
            buffer: vec![],
            incoming: Default::default(),
            error: None,
        }
    }

    pub fn should_send_heartbeat(&self, now: redux::Timestamp) -> bool {
        self.last_heartbeat_sent.is_none_or(|last_sent| {
            now.checked_sub(last_sent)
                .is_some_and(|dur| dur >= HEARTBEAT_INTERVAL)
        })
    }

    pub fn is_timed_out(&self, now: redux::Timestamp) -> bool {
        self.is_incoming
            && self.last_received.is_some_and(|last| {
                now.checked_sub(last)
                    .is_some_and(|elapsed| elapsed > INCOMING_HEARTBEAT_TIMEOUT)
            })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub enum RpcMessage {
    Handshake,
    Heartbeat,
    Query { header: QueryHeader, bytes: Data },
    Response { header: ResponseHeader, bytes: Data },
}

const HANDSHAKE_ID: P2pRpcId = P2pRpcId::from_le_bytes(*b"RPC\x00\x00\x00\x00\x00");

impl RpcMessage {
    pub fn into_bytes(self) -> Vec<u8> {
        let mut v = vec![0; 8];
        match self {
            Self::Handshake => {
                MessageHeader::Response(ResponseHeader { id: HANDSHAKE_ID })
                    .binprot_write(&mut v)
                    .unwrap_or_default();
                v.extend_from_slice(b"\x01");
            }
            Self::Heartbeat => {
                MessageHeader::Heartbeat
                    .binprot_write(&mut v)
                    .unwrap_or_default();
            }
            Self::Query { header, bytes } => {
                MessageHeader::Query(header)
                    .binprot_write(&mut v)
                    .unwrap_or_default();
                v.extend_from_slice(&bytes);
            }
            Self::Response { header, bytes } => {
                MessageHeader::Response(header)
                    .binprot_write(&mut v)
                    .unwrap_or_default();
                v.extend_from_slice(&bytes);
            }
        }

        let len_bytes = ((v.len() - 8) as u64).to_le_bytes();
        v[..8].clone_from_slice(&len_bytes);
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error, MallocSizeOf)]
pub enum P2pNetworkRpcError {
    #[error("error reading binprot message: {0}")]
    Binprot(String),
    #[error("message {0} with size {1} exceeds limit of {2}")]
    Limit(
        String,
        usize,
        #[ignore_malloc_size_of = "primitive"] Limit<usize>,
    ),
}

#[cfg(test)]
mod timeout_tests {
    use super::*;
    use redux::Timestamp;

    #[test]
    fn incoming_timeout_matches_ocaml_and_does_not_apply_to_our_outgoing_stream() {
        let start = Timestamp::ZERO + Duration::from_secs(100);
        let mut rpc = P2pNetworkRpcState::new(
            ConnectionAddr {
                sock_addr: "127.0.0.1:8302".parse().unwrap(),
                incoming: true,
            },
            1,
        );
        rpc.is_incoming = true;
        rpc.last_received = Some(start);
        // async_rpc_kernel.v0.14.0 connection.ml:heartbeat_now uses strict >.
        assert!(!rpc.is_timed_out(start + Duration::from_secs(30)));
        assert!(rpc.is_timed_out(start + Duration::from_secs(31)));
        assert!(!rpc.is_timed_out(Timestamp::ZERO + Duration::from_secs(99)));
        rpc.last_received = Some(start + Duration::from_secs(10));
        assert!(!rpc.is_timed_out(start + Duration::from_secs(31)));
        rpc.is_incoming = false;
        assert!(!rpc.is_timed_out(start + Duration::from_secs(120)));
    }
}
