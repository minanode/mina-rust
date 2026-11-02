//! Sync requests pipelined to a WebRTC peer (WEB-RELAY-RTT-THROUGHPUT).
//!
//! The RPC channel has one request slot, [`P2pChannelsRpcState`]'s `local`:
//! each request waits for the answer to the one before. A ledger sync asks
//! tens of thousands of questions of up to 64 accounts each (about 16 KB of
//! answer), so a web node 160 ms from its relay got about one answer a round
//! trip, 0.1 MB/s, and took tens of minutes to bootstrap. The 290 blocks
//! between the root and the best tip came the same way, one a round trip.
//!
//! A WebRTC peer has always taken up to
//! [`MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS`] requests at once (it
//! disconnects at one more), and answers each with its id. So a ledger query
//! or a block request may also go while the slot is taken, as long as the
//! slot and the pipeline together hold no more than that. The peer drops a
//! request from its count when it sends the answer, before we receive it, so
//! it never counts more than we do. libp2p peers, and every other request,
//! keep the single slot.

use serde::{Deserialize, Serialize};

use super::{P2pChannelsRpcState, P2pRpcId, P2pRpcRequest, MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pRpcPipeline {
    /// Whether the peer takes pipelined requests: a WebRTC peer.
    pub enabled: bool,
    /// Unanswered, oldest first.
    pub requests: Vec<P2pRpcPipelined>,
    /// The last one answered, for the answer's callback to find what was
    /// asked, as the slot's `Responded` state does.
    pub responded: Option<(P2pRpcId, Box<P2pRpcRequest>)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pRpcPipelined {
    /// When it was sent. The deadline that drops the peer counts from here
    /// ([`crate::channels::timed_out_request_expired`]).
    pub sent: redux::Timestamp,
    pub id: P2pRpcId,
    pub request: Box<P2pRpcRequest>,
    /// When it timed out once, and whoever asked was told. As in the slot,
    /// it may still be answered, and a second timeout disconnects the peer.
    pub timed_out: Option<redux::Timestamp>,
}

impl P2pRpcPipeline {
    pub fn for_peer(is_libp2p: bool) -> Self {
        Self {
            enabled: !is_libp2p,
            ..Default::default()
        }
    }

    /// The requests that may be pipelined: the sync's ledger queries and
    /// blocks, which it tracks per request id.
    pub fn takes(request: &P2pRpcRequest) -> bool {
        matches!(
            request,
            P2pRpcRequest::LedgerQuery(..) | P2pRpcRequest::Block(..)
        )
    }

    /// Whether one more request may go to the pipeline now, beside the
    /// slot's state `slot`. A free slot takes the request itself.
    pub fn has_room(&self, slot: &P2pChannelsRpcState) -> bool {
        self.enabled
            && slot.unanswered_local_rpc().is_some()
            && self.requests.len() + 1 < MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS
    }

    pub fn get(&self, id: P2pRpcId) -> Option<&P2pRpcPipelined> {
        self.requests.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: P2pRpcId) -> Option<&mut P2pRpcPipelined> {
        self.requests.iter_mut().find(|r| r.id == id)
    }

    /// Takes the request `id` out, answered, and keeps it as `responded`.
    pub fn answer(&mut self, id: P2pRpcId) -> bool {
        let Some(pos) = self.requests.iter().position(|r| r.id == id) else {
            return false;
        };
        let answered = self.requests.remove(pos);
        self.responded = Some((answered.id, answered.request));
        true
    }

    pub fn responded_request(&self, id: P2pRpcId) -> Option<&P2pRpcRequest> {
        self.responded
            .as_ref()
            .filter(|(rpc_id, _)| *rpc_id == id)
            .map(|(_, request)| request.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use mina_p2p_messages::v2::{LedgerHash, MinaLedgerSyncLedgerQueryStableV1};

    use super::*;
    use crate::channels::rpc::{P2pRpcLocalState, P2pRpcRemoteState};

    fn at(secs: u64) -> redux::Timestamp {
        redux::Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    fn query() -> P2pRpcRequest {
        P2pRpcRequest::LedgerQuery(
            LedgerHash::zero(),
            MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
        )
    }

    fn slot(local: P2pRpcLocalState) -> P2pChannelsRpcState {
        P2pChannelsRpcState::Ready {
            time: at(0),
            local,
            remote: P2pRpcRemoteState {
                pending_requests: Default::default(),
                last_responded: redux::Timestamp::ZERO,
            },
        }
    }

    fn pipelined(id: P2pRpcId) -> P2pRpcPipelined {
        P2pRpcPipelined {
            sent: at(1),
            id,
            request: Box::new(query()),
            timed_out: None,
        }
    }

    /// With the slot taken, a WebRTC peer takes ledger queries until five
    /// are unanswered in all, the most it accepts. Only ledger queries and
    /// blocks are pipelined.
    #[test]
    fn the_pipeline_fills_to_the_peers_limit() {
        let taken = slot(P2pRpcLocalState::Requested {
            time: at(1),
            id: 0,
            request: Box::new(query()),
        });
        let mut pipeline = P2pRpcPipeline::for_peer(false);
        for id in 1..MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS as u64 {
            assert!(pipeline.has_room(&taken));
            pipeline.requests.push(pipelined(id));
        }
        assert_eq!(pipeline.requests.len() + 1, 5);
        assert!(!pipeline.has_room(&taken));

        // A timed-out request still counts: the peer may still be on it.
        pipeline.requests[0].timed_out = Some(at(5));
        assert!(!pipeline.has_room(&taken));

        assert!(P2pRpcPipeline::takes(&query()));
        assert!(P2pRpcPipeline::takes(&P2pRpcRequest::Block(
            mina_p2p_messages::v2::StateHash::zero()
        )));
        assert!(!P2pRpcPipeline::takes(&P2pRpcRequest::BestTipWithProof));
    }

    /// A free slot takes the request itself; libp2p peers never pipeline.
    #[test]
    fn the_pipeline_is_only_behind_a_taken_slot_of_a_webrtc_peer() {
        let free = slot(P2pRpcLocalState::WaitingForRequest { time: at(0) });
        assert!(!P2pRpcPipeline::for_peer(false).has_room(&free));
        let taken = slot(P2pRpcLocalState::TimedOut {
            time: at(5),
            sent: at(1),
            id: 0,
            request: Box::new(query()),
        });
        assert!(P2pRpcPipeline::for_peer(false).has_room(&taken));
        assert!(!P2pRpcPipeline::for_peer(true).has_room(&taken));
    }

    #[test]
    fn an_answer_leaves_its_request_for_the_callback() {
        let mut pipeline = P2pRpcPipeline::for_peer(false);
        pipeline.requests.extend([pipelined(3), pipelined(4)]);
        assert!(!pipeline.answer(5));
        assert!(pipeline.answer(4));
        assert_eq!(pipeline.requests.len(), 1);
        assert!(pipeline.responded_request(4).is_some());
        assert!(pipeline.responded_request(3).is_none());
        assert!(pipeline.get(3).is_some());
    }
}
