use mina_core::ChainId;
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::{identity::PublicKey, PeerId};

use super::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkState {
    pub scheduler: P2pNetworkSchedulerState,
    /// Local correlation IDs for allowlisted inbound RPCs; wire IDs are
    /// scoped to a stream and can collide across concurrent streams.
    #[serde(default)]
    pub next_incoming_rpc_id: u64,
    #[serde(default)]
    pub serving_limits: crate::channels::rpc::ServingLimits,
    #[serde(default)]
    pub refusal_warnings: RpcRefusalWarnings,
    /// Length of the last staged-ledger answer we built for a libp2p peer,
    /// with its RPC framing. A staged-ledger query reserves this much of the
    /// send budgets until its answer is built
    /// ([`Self::staged_ledger_reservation`]).
    #[serde(default)]
    pub staged_ledger_answer_len: Option<usize>,
}

/// What a staged-ledger query reserves before we have built any staged-ledger
/// answer: what every one reserved before the reservation followed the answer.
pub const STAGED_LEDGER_FIRST_RESERVATION: usize = 64 * 1024 * 1024;

impl P2pNetworkState {
    /// The send budget an incoming staged-ledger query reserves until its
    /// answer is built: the length of the last answer we built, the best guess
    /// at the next one, as the scan state changes little from one root to the
    /// next. At most the largest answer we serve
    /// ([`crate::P2pLimits::rpc_served_staged_ledger`]), and
    /// [`STAGED_LEDGER_FIRST_RESERVATION`] before the first answer.
    ///
    /// The answer's real length is checked against the budgets once it is
    /// known, before the answer is encoded, so a guess that is too small costs
    /// only the work of building the answer.
    pub fn staged_ledger_reservation(&self, limits: &crate::P2pLimits) -> usize {
        let cap = match limits.rpc_served_staged_ledger() {
            crate::Limit::Some(cap) => cap,
            crate::Limit::Unlimited => usize::MAX,
        };
        self.staged_ledger_answer_len
            .unwrap_or(STAGED_LEDGER_FIRST_RESERVATION)
            .min(cap)
    }

    pub fn new(
        identity: PublicKey,
        addrs: Vec<Multiaddr>,
        known_peers: Vec<(PeerId, Multiaddr)>,
        chain_id: &ChainId,
        discovery: bool,
    ) -> Self {
        let peer_id = identity.peer_id();
        let pnet_key = chain_id.preshared_key();
        let discovery_state = discovery.then(|| {
            let mut routing_table = P2pNetworkKadRoutingTable::new(
                P2pNetworkKadEntry::new(peer_id, addrs).expect("valid peer_id"),
            );
            routing_table.extend(known_peers.into_iter().map(|(peer_id, maddr)| {
                P2pNetworkKadEntry::new(peer_id, vec![maddr]).expect("valid known peer")
            }));
            P2pNetworkKadState {
                routing_table,
                ..Default::default()
            }
        });

        P2pNetworkState {
            next_incoming_rpc_id: 0,
            serving_limits: Default::default(),
            refusal_warnings: Default::default(),
            staged_ledger_answer_len: None,
            scheduler: P2pNetworkSchedulerState {
                interfaces: Default::default(),
                listeners: Default::default(),
                local_pk: identity,
                pnet_key,
                connections: Default::default(),
                broadcast_state: Default::default(),
                identify_state: Default::default(),
                discovery_state,
                rpc_incoming_streams: Default::default(),
                rpc_outgoing_streams: Default::default(),
            },
        }
    }

    pub fn find_rpc_state(&self, a: &P2pNetworkRpcAction) -> Option<&P2pNetworkRpcState> {
        match a.stream_id() {
            RpcStreamId::Exact(stream_id) => self
                .scheduler
                .rpc_incoming_streams
                .get(a.peer_id())
                .and_then(|cn| cn.get(&stream_id))
                .or_else(|| {
                    self.scheduler
                        .rpc_outgoing_streams
                        .get(a.peer_id())
                        .and_then(|cn| cn.get(&stream_id))
                }),
            RpcStreamId::WithQuery(id) => self
                .scheduler
                .rpc_incoming_streams
                .get(a.peer_id())
                .and_then(|streams| {
                    streams.iter().find_map(|(_, state)| {
                        if state
                            .pending
                            .as_ref()
                            .is_some_and(|query_header| query_header.id == id)
                            || state.pending_incoming.contains_key(&id)
                        {
                            Some(state)
                        } else {
                            None
                        }
                    })
                }),
            RpcStreamId::AnyIncoming => self
                .scheduler
                .rpc_incoming_streams
                .get(a.peer_id())
                .and_then(|stream| stream.first_key_value())
                .map(|(_k, v)| v),
            RpcStreamId::AnyOutgoing => {
                if let Some(streams) = self.scheduler.rpc_outgoing_streams.get(a.peer_id()) {
                    if let Some((k, _)) = streams.first_key_value() {
                        return Some(streams.get(k).expect("checked above"));
                    }
                }

                None
            }
        }
    }
}
