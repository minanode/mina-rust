use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    time::Duration,
};

use malloc_size_of_derive::MallocSizeOf;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    connection::outgoing::P2pConnectionOutgoingInitOpts, P2pNetworkKadKey, P2pNetworkKadKeyError,
    P2pNetworkKadLatestRequestPeers, PeerId,
};

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
pub struct P2pNetworkKadBootstrapState {
    /// Key that is used to request closest peers. Usually self peer_id.
    pub key: PeerId,
    /// Kademlia key, `sha265(self.key)`.
    pub kademlia_key: P2pNetworkKadKey,
    /// Peers that already been contacted (successfully or not) for FIND_NODE.
    #[with_malloc_size_of_func = "measurement::peer_id_map"]
    pub processed_peers: BTreeSet<PeerId>,
    /// Ongoing FIND_NODE requests.
    ///
    /// TODO: replace with something more lightweight.
    #[with_malloc_size_of_func = "measurement::requests_map"]
    pub requests: BTreeMap<PeerId, P2pNetworkKadBoostrapRequestState>,
    /// Number of successful requests
    pub successful_requests: usize,
    /// Bootstrap requests statistics.
    pub stats: P2pNetworkKadBootstrapStats,
    /// Constructing request
    pub peer_id_req_vec: Vec<(PeerId, P2pNetworkKadBoostrapRequestState)>,
    /// Number of requests to construct
    pub requests_number: usize,
    /// When this lookup started.
    #[serde(default)]
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub started_at: Option<Timestamp>,
}

impl P2pNetworkKadBootstrapState {
    /// A lookup ends after this many successful `FIND_NODE` requests.
    pub const SUCCESSFUL_REQUESTS: usize = 20;
    /// A lookup that has not ended after this long is ended: a request whose
    /// peer never answers has no timeout of its own, and before I18b it kept
    /// Kademlia bootstrapping, and so not discovering, for good.
    pub const TIMEOUT: Duration = Duration::from_secs(120);

    pub fn new(key: PeerId) -> Result<Self, P2pNetworkKadKeyError> {
        Ok(P2pNetworkKadBootstrapState {
            key,
            kademlia_key: key.try_into()?,
            processed_peers: BTreeSet::new(),
            requests: BTreeMap::new(),
            successful_requests: 0,
            stats: Default::default(),
            peer_id_req_vec: vec![],
            requests_number: 0,
            started_at: None,
        })
    }

    /// Whether the lookup has run for [`Self::TIMEOUT`] at `now`.
    pub fn is_timed_out(&self, now: Timestamp) -> bool {
        self.started_at
            .is_some_and(|started| now.checked_sub(started).unwrap_or_default() >= Self::TIMEOUT)
    }

    /// Whether the lookup is done: it has had enough successful requests, or
    /// there is no one left to ask, and no request is in flight or being
    /// assembled.
    pub fn is_done(&self) -> bool {
        self.requests.is_empty() && !self.is_assembling()
    }

    /// Whether a batch of requests (`CreateRequests`) is still being
    /// assembled: some of its peers have yet to be appended, or appended
    /// ones have yet to be finalized.
    pub fn is_assembling(&self) -> bool {
        self.requests_number > 0 || !self.peer_id_req_vec.is_empty()
    }

    /// Whether this lookup has asked `peer_id` already, or holds it in the
    /// batch being assembled.
    pub fn is_known(&self, peer_id: &PeerId) -> bool {
        self.processed_peers.contains(peer_id)
            || self.peer_id_req_vec.iter().any(|(id, _)| id == peer_id)
    }

    /// What the lookup does after one of its requests ended: ask more peers
    /// while it has had fewer than [`Self::SUCCESSFUL_REQUESTS`] successful
    /// requests, and otherwise end once the last request in flight has
    /// ended; nothing while a batch is being assembled.
    pub fn next_step(&self) -> P2pNetworkKadBootstrapNextStep {
        if self.is_assembling() {
            P2pNetworkKadBootstrapNextStep::Wait
        } else if self.successful_requests < Self::SUCCESSFUL_REQUESTS {
            P2pNetworkKadBootstrapNextStep::AskMore
        } else if self.is_done() {
            P2pNetworkKadBootstrapNextStep::Finish
        } else {
            P2pNetworkKadBootstrapNextStep::Wait
        }
    }

    pub fn request(&self, peer_id: &PeerId) -> Option<&P2pNetworkKadBoostrapRequestState> {
        self.requests.get(peer_id)
    }
}

/// What a lookup does after one of its requests ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P2pNetworkKadBootstrapNextStep {
    /// Ask the peers not yet asked (`CreateRequests`).
    AskMore,
    /// End the lookup (`BootstrapFinished`).
    Finish,
    /// Nothing now: a batch is being assembled, or requests are in flight.
    Wait,
}

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
pub struct P2pNetworkKadBoostrapRequestState {
    /// Address that is used for the current connection.
    // TODO: generalize to DNS addrs
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub addr: SocketAddr,
    /// When connection to the peer was initiated.
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub time: Timestamp,
    /// Addresses yet to be used, if current connection will fail.
    // TODO: use Multiaddr
    #[with_malloc_size_of_func = "measurement::socket_addr_vec"]
    pub addrs_to_use: Vec<SocketAddr>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, MallocSizeOf)]
pub struct P2pNetworkKadBootstrapStats {
    pub requests: Vec<P2pNetworkKadBootstrapRequestStat>,
}

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
#[serde(tag = "type")]
pub enum P2pNetworkKadBootstrapRequestStat {
    Ongoing(P2pNetworkKadBootstrapOngoingRequest),
    Successful(P2pNetworkKadBootstrapSuccessfulRequest),
    Failed(P2pNetworkKadBootstrapFailedRequest),
}

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
pub struct P2pNetworkKadBootstrapOngoingRequest {
    pub peer_id: PeerId,
    pub address: P2pConnectionOutgoingInitOpts,
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub start: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
pub struct P2pNetworkKadBootstrapSuccessfulRequest {
    pub peer_id: PeerId,
    pub address: P2pConnectionOutgoingInitOpts,
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub start: Timestamp,
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub finish: Timestamp,
    pub closest_peers: P2pNetworkKadLatestRequestPeers,
}

#[derive(Clone, Debug, Serialize, Deserialize, MallocSizeOf)]
pub struct P2pNetworkKadBootstrapFailedRequest {
    pub peer_id: PeerId,
    pub address: P2pConnectionOutgoingInitOpts,
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub start: Timestamp,
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub finish: Timestamp,
    pub error: String,
}

mod measurement {
    use std::{
        collections::{BTreeMap, BTreeSet},
        mem,
        net::SocketAddr,
    };

    use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};

    use super::P2pNetworkKadBoostrapRequestState;
    use crate::PeerId;

    pub fn socket_addr_vec(val: &Vec<SocketAddr>, _ops: &mut MallocSizeOfOps) -> usize {
        val.capacity() * mem::size_of::<SocketAddr>()
    }

    pub fn peer_id_map(val: &BTreeSet<PeerId>, _ops: &mut MallocSizeOfOps) -> usize {
        val.len() * mem::size_of::<PeerId>()
    }

    pub fn requests_map(
        val: &BTreeMap<PeerId, P2pNetworkKadBoostrapRequestState>,
        ops: &mut MallocSizeOfOps,
    ) -> usize {
        val.iter()
            .map(|(k, v)| mem::size_of_val(k) + mem::size_of_val(v) + v.size_of(ops))
            .sum()
    }
}
