use std::{collections::BTreeSet, net::IpAddr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    channels::ChannelId, connection::outgoing::P2pConnectionOutgoingInitOpts, identity::PublicKey,
};

pub const DEVNET_SEEDS: &[&str] = &[
    "/dns4/seed-1.devnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs",
    "/dns4/seed-2.devnet.gcp.o1test.net/tcp/32012/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag",
    "/dns4/seed-3.devnet.gcp.o1test.net/tcp/32013/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv",
];

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pConfig {
    /// TCP port where libp2p is listening incoming connections.
    pub libp2p_port: Option<u16>,
    /// The HTTP port where signaling server is listening SDP offers and SDP answers.
    pub listen_port: Option<u16>,
    /// The public key used for authentication all p2p communication.
    pub identity_pub_key: PublicKey,
    /// A list addresses of seed nodes.
    pub initial_peers: Vec<P2pConnectionOutgoingInitOpts>,
    /// External addresses
    pub external_addrs: Vec<IpAddr>,

    pub enabled_channels: BTreeSet<ChannelId>,

    pub timeouts: P2pTimeouts,

    pub limits: P2pLimits,

    /// Use peers discovery.
    pub peer_discovery: bool,

    pub meshsub: P2pMeshsubConfig,

    /// OCaml catch-up responders. Enabled for all peers by default; a nonempty
    /// peer set restricts serving for isolated tests or private deployments.
    #[serde(default)]
    pub catchup_rpc: P2pCatchupRpcConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pCatchupRpcConfig {
    pub enabled: bool,
    pub peers: BTreeSet<crate::PeerId>,
}

impl Default for P2pCatchupRpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            peers: BTreeSet::new(),
        }
    }
}

impl P2pCatchupRpcConfig {
    pub fn allows(&self, peer: &crate::PeerId) -> bool {
        self.enabled && (self.peers.is_empty() || self.peers.contains(peer))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pMeshsubConfig {
    /// Unix time. Used as an initial nonce for pubsub.
    pub initial_time: Duration,

    pub outbound_degree_desired: usize,
    pub outbound_degree_low: usize,
    pub outbound_degree_high: usize,
    pub mcache_len: usize,
}

impl Default for P2pMeshsubConfig {
    fn default() -> Self {
        P2pMeshsubConfig {
            initial_time: Duration::ZERO,
            outbound_degree_desired: 6,
            outbound_degree_low: 4,
            outbound_degree_high: 12,
            mcache_len: 256,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pTimeouts {
    pub incoming_connection_timeout: Option<Duration>,
    pub outgoing_connection_timeout: Option<Duration>,
    pub reconnect_timeout: Option<Duration>,
    pub incoming_error_reconnect_timeout: Option<Duration>,
    pub outgoing_error_reconnect_timeout: Option<Duration>,
    pub best_tip_with_proof: Option<Duration>,
    pub ledger_query: Option<Duration>,
    pub staged_ledger_aux_and_pending_coinbases_at_block: Option<Duration>,
    pub block: Option<Duration>,
    pub snark: Option<Duration>,
    pub transaction: Option<Duration>,
    pub initial_peers: Option<Duration>,
    pub kademlia_bootstrap: Option<Duration>,
    pub kademlia_initial_bootstrap: Option<Duration>,
    pub select: Option<Duration>,
    /// How long a stream the peer opened may take to negotiate its protocol
    /// before we reset it. go-libp2p, which OCaml nodes run, gives the
    /// responder 10 s (`DefaultNegotiationTimeout` in
    /// `p2p/host/basic/basic_host.go`, v0.27.7) and opens a stream before it
    /// has anything to send on it, so a busy peer can be silent for seconds.
    pub select_incoming_stream: Option<Duration>,
    pub pnet: Option<Duration>,
    /// Least time between two best tip requests to one libp2p peer, or to one
    /// IP ([`crate::channels::rpc::P2pBestTipRequestBudget`]). Nodes that
    /// share a public IP share OCaml's budget, so each should use a multiple
    /// of the default 20 s.
    #[serde(default)]
    pub best_tip_request_interval: Option<Duration>,
    /// The longest wait before dialling a libp2p peer again, before jitter
    /// ([`crate::connection::outgoing::P2pDialBackoff`]). The waits start at
    /// `reconnect_timeout` after a disconnection and at
    /// `outgoing_error_reconnect_timeout` after a failed dial, and double.
    #[serde(default)]
    pub reconnect_backoff_max: Option<Duration>,
    /// How long a libp2p connection must stay ready for its end to reset the
    /// peer's backoff. OCaml's connection manager trims new connections once
    /// its 15 s grace period is over, on a 10 s tick, so a shorter connection
    /// says the peer doesn't want us.
    #[serde(default)]
    pub healthy_connection: Option<Duration>,
    /// How long a libp2p peer id isn't dialled at an address that answered
    /// the handshake with another peer id.
    #[serde(default)]
    pub stale_peer_id: Option<Duration>,
}

fn from_env_or(name: &str, default: Option<Duration>) -> Option<Duration> {
    None.or_else(|| {
        let val = std::env::var(name).ok()?.to_ascii_lowercase();
        Some(match val.as_ref() {
            "none" => None,
            s => Some(Duration::from_secs(s.parse().ok()?)),
        })
    })
    .unwrap_or(default)
}

impl Default for P2pTimeouts {
    fn default() -> Self {
        Self {
            incoming_connection_timeout: from_env_or(
                "INCOMING_CONNECTION_TIMEOUT",
                Some(Duration::from_secs(30)),
            ),
            outgoing_connection_timeout: from_env_or(
                "OUTGOING_CONNECTION_TIMEOUT",
                Some(Duration::from_secs(15)),
            ),
            reconnect_timeout: from_env_or("RECONNECT_TIMEOUT", Some(Duration::from_secs(30))),
            incoming_error_reconnect_timeout: from_env_or(
                "INCOMING_ERROR_RECONNECT_TIMEOUT",
                Some(Duration::from_secs(30)),
            ),
            outgoing_error_reconnect_timeout: from_env_or(
                "OUTGOING_ERROR_RECONNECT_TIMEOUT",
                Some(Duration::from_secs(30)),
            ),
            best_tip_with_proof: from_env_or(
                "BEST_TIP_WITH_PROOF_TIMEOUT",
                Some(Duration::from_secs(15)),
            ),
            ledger_query: from_env_or("LEDGER_QUERY_TIMEOUT", Some(Duration::from_secs(4))),
            staged_ledger_aux_and_pending_coinbases_at_block: from_env_or(
                "STAGED_LEDGER_AUX_AND_PENDING_COINBASES_AT_BLOCK_TIMEOUT",
                Some(Duration::from_secs(180)),
            ),
            block: from_env_or("BLOCK_TIMEOUT", Some(Duration::from_secs(8))),
            snark: from_env_or("SNARK_TIMEOUT", Some(Duration::from_secs(8))),
            transaction: from_env_or("TRANSACTION_TIMEOUT", Some(Duration::from_secs(8))),
            initial_peers: from_env_or("INITIAL_PEERS_TIMEOUT", Some(Duration::from_secs(5))),
            kademlia_bootstrap: from_env_or(
                "KADEMLIA_BOOTSTRAP_TIMEOUT",
                Some(Duration::from_secs(60)),
            ),
            kademlia_initial_bootstrap: from_env_or(
                "KADEMLIA_INITIAL_BOOTSTRAP_TIMEOUT",
                Some(Duration::from_secs(5)),
            ),
            select: from_env_or("SELECT_TIMEOUT", Some(Duration::from_secs(5))),
            select_incoming_stream: from_env_or(
                "INCOMING_STREAM_SELECT_TIMEOUT",
                Some(Duration::from_secs(10)),
            ),
            pnet: from_env_or("PNET_TIMEOUT", Some(Duration::from_secs(2))),
            best_tip_request_interval: from_env_or(
                "BEST_TIP_REQUEST_INTERVAL",
                Some(Duration::from_secs(20)),
            ),
            reconnect_backoff_max: from_env_or(
                "RECONNECT_BACKOFF_MAX",
                Some(Duration::from_secs(10 * 60)),
            ),
            healthy_connection: from_env_or(
                "HEALTHY_CONNECTION_TIME",
                Some(Duration::from_secs(60)),
            ),
            stale_peer_id: from_env_or("STALE_PEER_ID_TIMEOUT", Some(Duration::from_secs(60 * 60))),
        }
    }
}

impl P2pTimeouts {
    pub fn without_rpc() -> Self {
        Self {
            best_tip_with_proof: None,
            ledger_query: None,
            staged_ledger_aux_and_pending_coinbases_at_block: None,
            block: None,
            snark: None,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, derive_more::Display, Default)]
pub enum Limit<T> {
    #[display(fmt = "{}", _0)]
    Some(T),
    #[default]
    #[display(fmt = "unlimited")]
    Unlimited,
}

impl<T> Limit<T> {
    pub fn map<F, O>(self, f: F) -> Limit<O>
    where
        F: FnOnce(T) -> O,
    {
        match self {
            Limit::Some(t) => Limit::Some(f(t)),
            Limit::Unlimited => Limit::Unlimited,
        }
    }
}

macro_rules! impls {
    ($ty:ty) => {
        impl From<Option<$ty>> for Limit<$ty> {
            fn from(value: Option<$ty>) -> Self {
                match value {
                    Some(v) => Limit::Some(v),
                    None => Limit::Unlimited,
                }
            }
        }

        impl From<Limit<$ty>> for Option<$ty> {
            fn from(value: Limit<$ty>) -> Self {
                match value {
                    Limit::Some(v) => Some(v),
                    Limit::Unlimited => None,
                }
            }
        }

        impl std::cmp::PartialEq<$ty> for Limit<$ty> {
            fn eq(&self, other: &$ty) -> bool {
                match self {
                    Limit::Some(v) => v.eq(other),
                    Limit::Unlimited => false,
                }
            }
        }

        impl std::cmp::PartialEq<Limit<$ty>> for $ty {
            fn eq(&self, other: &Limit<$ty>) -> bool {
                match other {
                    Limit::Some(other) => self.eq(other),
                    Limit::Unlimited => false,
                }
            }
        }

        impl std::cmp::PartialEq<Limit<$ty>> for Limit<$ty> {
            fn eq(&self, other: &Limit<$ty>) -> bool {
                match (self, other) {
                    (Limit::Some(this), Limit::Some(other)) => this.eq(other),
                    (Limit::Unlimited, Limit::Unlimited) => true,
                    _ => false,
                }
            }
        }

        impl std::cmp::Eq for Limit<$ty> {}

        impl std::cmp::PartialOrd<$ty> for Limit<$ty> {
            fn partial_cmp(&self, other: &$ty) -> Option<std::cmp::Ordering> {
                match self {
                    Limit::Some(v) => v.partial_cmp(other),
                    Limit::Unlimited => Some(std::cmp::Ordering::Greater),
                }
            }
        }

        impl std::cmp::PartialOrd<Limit<$ty>> for $ty {
            fn partial_cmp(&self, other: &Limit<$ty>) -> Option<std::cmp::Ordering> {
                match other {
                    Limit::Some(other) => self.partial_cmp(other),
                    Limit::Unlimited => Some(std::cmp::Ordering::Less),
                }
            }
        }
    };
}

impls!(usize);
impls!(std::time::Duration);

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct P2pLimits {
    max_peers: Limit<usize>,
    min_peers_in_state: Limit<usize>,
    max_peers_in_state: Limit<usize>,
    max_streams: Limit<usize>,
    yamux_message_size: Limit<usize>,
    yamux_pending_outgoing_per_peer: Limit<usize>,
    #[serde(default = "P2pLimits::default_yamux_pending_outgoing_total")]
    yamux_pending_outgoing_total: Limit<usize>,
    #[serde(default = "P2pLimits::default_yamux_pending_outgoing_peer_share")]
    yamux_pending_outgoing_peer_share: Limit<usize>,
    #[serde(default = "P2pLimits::default_yamux_pending_outgoing_fair_share")]
    yamux_pending_outgoing_fair_share: Limit<usize>,
    #[serde(default = "P2pLimits::default_yamux_pending_outgoing_reserve")]
    yamux_pending_outgoing_reserve: Limit<usize>,
    #[serde(default = "P2pLimits::default_rpc_inflight_peer_share")]
    rpc_inflight_peer_share: Limit<usize>,

    identify_message: Limit<usize>,
    kademlia_request: Limit<usize>,
    kademlia_response: Limit<usize>,

    rpc_service_message: Limit<usize>,
    rpc_query: Limit<usize>,
    rpc_get_best_tip: Limit<usize>,
    rpc_answer_sync_ledger_query: Limit<usize>,
    rpc_get_staged_ledger: Limit<usize>,
    rpc_get_transition_chain: Limit<usize>,
    rpc_get_some_initial_peers: Limit<usize>,

    #[serde(default = "P2pLimits::default_max_redials_per_minute")]
    max_redials_per_minute: Limit<usize>,
    #[serde(default = "P2pLimits::default_seed_reconnect_below")]
    seed_reconnect_below: Limit<usize>,
    #[serde(default = "P2pLimits::default_max_dials_per_minute")]
    max_dials_per_minute: Limit<usize>,
    #[serde(default = "P2pLimits::default_libp2p_target_peers")]
    libp2p_target_peers: Limit<usize>,
}

macro_rules! limit {
    (#[$meta:meta] $limit:ident) => {
        #[$meta]
        pub fn $limit(&self) -> Limit<usize> {
            self.$limit
        }
    };

    (#[$meta:meta] $limit:ident, #[$setter_meta:meta] $setter:ident) => {
        limit!(#[$meta] $limit);

        #[$setter_meta]
        pub fn $setter<T: Into<Limit<usize>>>(mut self, $limit: T) -> Self {
            self.$limit = $limit.into();
            self
        }
    };

    (#[$meta:meta] $limit:ident(&$self:ident): $expr:expr) => {
        #[$meta]
        pub fn $limit(&$self) -> Limit<usize> {
            $expr
        }
    };
}

impl P2pLimits {
    limit!(
        /// Maximum number of peers.
        max_peers,
        /// Sets maximum number of peers.
        with_max_peers
    );
    limit!(
        /// Minimum number of peers in state
        min_peers_in_state
    );
    limit!(
        /// Maximum number of peers in state
        max_peers_in_state
    );
    limit!(
        /// Maximum number of streams from a peer.
        max_streams,
        /// Sets the maximum number of streams that a peer is allowed to open simultaneously.
        with_max_streams
    );
    limit!(
        /// Maximum number of streams from a peer.
        yamux_message_size,
        /// Sets the maximum number of streams that a peer is allowed to open simultaneously.
        with_yamux_message_size
    );
    limit!(
        /// Maximum number of streams from a peer.
        yamux_pending_outgoing_per_peer,
        /// Sets the maximum number of streams that a peer is allowed to open simultaneously.
        with_yamux_pending_outgoing_per_peer
    );
    limit!(
        /// Node-wide bytes queued behind peers' Yamux receive windows.
        yamux_pending_outgoing_total,
        /// Sets the node-wide queued response byte budget.
        with_yamux_pending_outgoing_total
    );
    limit!(
        /// Maximum queued response bytes for one authenticated peer.
        yamux_pending_outgoing_peer_share,
        /// Sets one peer's share of the queued response budget.
        with_yamux_pending_outgoing_peer_share
    );
    limit!(
        /// Fair share available even while other peers have borrowed spare capacity.
        yamux_pending_outgoing_fair_share,
        /// Sets the share reserved for a newly serving peer.
        with_yamux_pending_outgoing_fair_share
    );
    limit!(
        /// Capacity that peers above their fair share cannot borrow.
        yamux_pending_outgoing_reserve,
        /// Sets the node-wide fair-share reserve.
        with_yamux_pending_outgoing_reserve
    );
    limit!(
        /// Expected response bytes of dispatched work for one peer.
        rpc_inflight_peer_share,
        /// Sets one peer's in-flight RPC work budget.
        with_rpc_inflight_peer_share
    );

    fn outgoing_total_and_fair_allow(&self, total: usize, peer: usize, added: usize) -> bool {
        let next_total = total.saturating_add(added);
        let next_peer = peer.saturating_add(added);
        let borrowed_region = match (
            self.yamux_pending_outgoing_total(),
            self.yamux_pending_outgoing_reserve(),
        ) {
            (Limit::Some(total), Limit::Some(reserve)) => {
                next_total > total.saturating_sub(reserve)
            }
            _ => false,
        };
        next_total <= self.yamux_pending_outgoing_total()
            && (!borrowed_region || next_peer <= self.yamux_pending_outgoing_fair_share())
    }

    pub fn rpc_work_budget_allows(&self, total: usize, peer: usize, added: usize) -> bool {
        peer.saturating_add(added) <= self.rpc_inflight_peer_share()
            && self.outgoing_total_and_fair_allow(total, peer, added)
    }

    pub fn outgoing_queue_budget_allows(
        &self,
        total: usize,
        peer: usize,
        peer_queued: usize,
        added: usize,
    ) -> bool {
        peer_queued.saturating_add(added) <= self.yamux_pending_outgoing_peer_share()
            && self.outgoing_total_and_fair_allow(total, peer, added)
    }

    fn default_yamux_pending_outgoing_total() -> Limit<usize> {
        Limit::Some(256 * 1024 * 1024)
    }

    fn default_yamux_pending_outgoing_peer_share() -> Limit<usize> {
        Limit::Some(64 * 1024 * 1024)
    }

    fn default_yamux_pending_outgoing_fair_share() -> Limit<usize> {
        Limit::Some(32 * 1024 * 1024)
    }

    fn default_yamux_pending_outgoing_reserve() -> Limit<usize> {
        Limit::Some(64 * 1024 * 1024)
    }

    fn default_rpc_inflight_peer_share() -> Limit<usize> {
        Limit::Some(128 * 1024 * 1024)
    }

    limit!(
        /// Minimum number of peers.
        min_peers(&self): self.max_peers.map(|v| (v / 2).max(3).min(v))
    );

    limit!(
        /// Above this limit, peers will be randomly disconnected to free up space.
        max_stable_peers(&self): self.max_peers.map(|v| v.saturating_mul(8).saturating_div(10))
    );

    limit!(
        /// Maximum number of connections.
        max_connections(&self): self.max_peers.map(|v| v + 10)
    );

    limit!(
        /// Maximum length of Identify message.
        identify_message
    );
    limit!(
        /// Maximum length of Kademlia request message.
        kademlia_request
    );
    limit!(
        /// Maximum length of Kademlia response message.
        kademlia_response
    );

    limit!(
        #[doc = "RPC service message"]
        rpc_service_message
    );
    limit!(
        #[doc = "RPC query"]
        rpc_query
    );
    limit!(
        #[doc = "RPC get_best_tip"]
        rpc_get_best_tip
    );
    limit!(
        #[doc = "RPC answer_sync_ledger_query"]
        rpc_answer_sync_ledger_query
    );
    limit!(
        #[doc = "RPC get_staged_ledger"]
        rpc_get_staged_ledger
    );
    limit!(
        #[doc = "RPC get_transition_chain"]
        rpc_get_transition_chain
    );
    limit!(
        #[doc = "RPC some_initial_peers"]
        rpc_get_some_initial_peers
    );

    limit!(
        #[doc = "Dials to libp2p peer ids or addresses we dialled before, at most, in any minute ([`crate::connection::outgoing::P2pDialBackoff`])."]
        max_redials_per_minute,
        /// Sets the most redials in any minute.
        with_max_redials_per_minute
    );
    limit!(
        #[doc = "Seeds (the initial peers) are dialled again for a connection, and not only by discovery, while fewer than this many libp2p peers that aren't seeds are ready. OCaml dials its seeds once at start, and again only when its DHT routing table is empty."]
        seed_reconnect_below,
        /// Sets how few ready non-seed peers make us dial seeds again.
        with_seed_reconnect_below
    );

    limit!(
        #[doc = "Libp2p dials of any kind, first dials to new peers and redials, at most, in any minute ([`crate::connection::outgoing::P2pDialBackoff`]). Redials are also held to [`Self::max_redials_per_minute`]."]
        max_dials_per_minute,
        /// Sets the most libp2p dials in any minute.
        with_max_dials_per_minute
    );
    limit!(
        #[doc = "How many libp2p peers we keep: below it we dial discovered peers, never-tried ones first, and look for more with Kademlia. OCaml's helper dials what it discovers while it has fewer connections than its low-water mark, `min_connections` (20)."]
        libp2p_target_peers,
        /// Sets how many libp2p peers we keep.
        with_libp2p_target_peers
    );

    const fn default_max_redials_per_minute() -> Limit<usize> {
        Limit::Some(20)
    }

    const fn default_seed_reconnect_below() -> Limit<usize> {
        Limit::Some(3)
    }

    /// Above the 94 dials of the busiest minute of m8's bootstrap (74 first
    /// dials and 20 redials), so it doesn't slow a bootstrap, and a ceiling
    /// on any runaway: in steady state first dials need newly discovered
    /// peers and redials are held to 20 a minute.
    const fn default_max_dials_per_minute() -> Limit<usize> {
        Limit::Some(100)
    }

    /// OCaml's `min_connections`, the low-water mark below which its helper
    /// dials the peers it discovers.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/cli_lib/default.ml#L4-L6>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L86-L93>
    const fn default_libp2p_target_peers() -> Limit<usize> {
        Limit::Some(20)
    }
}

impl Default for P2pLimits {
    fn default() -> Self {
        let max_peers = Limit::Some(100);
        let min_peers_in_state = Limit::Some(50);
        let max_peers_in_state = Limit::Some(100);
        // OCaml's helper uses go-libp2p's Yamux transport. go-yamux/v4
        // defaults to 1,000 incoming streams and 256 pending accepts;
        // go-libp2p v0.27.7 overrides the stream limit to MaxUint32 and
        // relies on its resource manager. OCaml ledger sync sends up to 100
        // queries in parallel. Allow those and a little protocol headroom,
        // while retaining a much tighter per-peer cap than go-libp2p.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/libp2p_helper/src/codanet.go#L717>
        // go-libp2p: <https://github.com/libp2p/go-libp2p/blob/v0.27.7/p2p/muxer/yamux/transport.go#L14-L31>
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/mina_networking.ml#L441-L453>
        // go-yamux: <https://github.com/libp2p/go-yamux/blob/v4.0.0/mux.go#L62-L82>
        let max_streams = Limit::Some(128);
        // 256 MiB
        let yamux_message_size = Limit::Some(0x10000000);

        let identify_message = Limit::Some(0x1000);
        // OCaml's go-libp2p helper uses network.MessageSizeMax (4 MiB) for
        // varint-framed Kademlia messages in both directions.
        // https://github.com/libp2p/go-libp2p/blob/master/core/network/network.go
        let kademlia_request = Limit::Some(1 << 22);
        let kademlia_response = Limit::Some(1 << 22);

        let rpc_service_message = Limit::Some(7); // 7 for handshake, 1 for heartbeat

        // A query on a stream the peer opened. OCaml's largest is
        // `get_ancestry`'s, a consensus state and a state hash (about 600
        // bytes); one over the limit is dropped and its stream stalls.
        let rpc_query = Limit::Some(4096);

        let rpc_get_best_tip = Limit::Some(3_500_000); // 3182930 as observed, may vary

        // The largest answer is the accounts of a subtree of height 6, 64
        // accounts. A Mesa account is at most 3450 bytes in bin_prot (a zkApp
        // with a 32 wide app state, a verification key and a 255 byte URI), so
        // 220866 bytes with the RPC framing (mina-node's
        // largest_account_query_answer_fits_the_rpc_limit). Observed: 124823 on
        // mainnet; on devnet, 5 of the 1435 subtrees of the Mesa genesis
        // staking ledger hold over 200 KB of accounts (203344 at most), and
        // they are the 5 a devnet node's staking ledger sync never got with a
        // 200 KB limit (P1c).
        //
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_ledger/sync_ledger.ml#L29>
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L321-L333>
        let rpc_answer_sync_ledger_query = Limit::Some(1_000_000);
        let rpc_get_staged_ledger = Limit::Some(400_000_000); // 59286608 as observed, may go higher
        let rpc_get_transition_chain = Limit::Some(3_500_000); // 2979112 as observed
        let rpc_get_some_initial_peers = Limit::Some(32_000); // TODO: calculate
        let max_redials_per_minute = Self::default_max_redials_per_minute();
        let seed_reconnect_below = Self::default_seed_reconnect_below();
        let max_dials_per_minute = Self::default_max_dials_per_minute();
        let libp2p_target_peers = Self::default_libp2p_target_peers();

        Self {
            max_peers,
            min_peers_in_state,
            max_peers_in_state,
            max_streams,
            yamux_message_size,
            yamux_pending_outgoing_per_peer: rpc_get_staged_ledger,
            yamux_pending_outgoing_total: Self::default_yamux_pending_outgoing_total(),
            yamux_pending_outgoing_peer_share: Self::default_yamux_pending_outgoing_peer_share(),
            yamux_pending_outgoing_fair_share: Self::default_yamux_pending_outgoing_fair_share(),
            yamux_pending_outgoing_reserve: Self::default_yamux_pending_outgoing_reserve(),
            rpc_inflight_peer_share: Self::default_rpc_inflight_peer_share(),

            identify_message,
            kademlia_request,
            kademlia_response,

            rpc_service_message,
            rpc_query,
            rpc_get_best_tip,
            rpc_answer_sync_ledger_query,
            rpc_get_staged_ledger,
            rpc_get_transition_chain,
            rpc_get_some_initial_peers,
            max_redials_per_minute,
            seed_reconnect_below,
            max_dials_per_minute,
            libp2p_target_peers,
        }
    }
}

#[cfg(test)]
mod d4f_tests {
    use super::*;

    #[test]
    fn public_yamux_cap_admits_ocaml_bootstrap_burst_but_stays_finite() {
        // D4 observed at least 11 live incoming RPC streams; OCaml may send
        // 100 ledger queries in parallel. Retain 28 slots for other streams.
        let Limit::Some(limit) = P2pLimits::default().max_streams() else {
            panic!("incoming streams must remain bounded");
        };
        assert_eq!(limit, 128);
        assert!(100 <= limit);
        assert!(129 > limit);
    }
}

#[cfg(test)]
mod tests {

    use super::{Limit, P2pLimits};

    /// The `get_ancestry` query an OCaml node sends while bootstrapping, built
    /// from the best tip of a Mesa mainnet node, fits the query limit. It
    /// didn't fit the old 256 bytes, so our node never answered it (I17).
    #[test]
    fn rpc_query_limit_fits_ocaml_get_ancestry() {
        use binprot::{BinProtRead, BinProtWrite};
        use mina_p2p_messages::{
            rpc::{GetAncestryV2, GetAncestryV2Query, GetBestTipV2Response, WithHashV1},
            rpc_kernel::{Message, NeedsLength, Query, ResponsePayload, RpcMethod},
        };

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
        );
        let bytes = std::fs::read(path).unwrap();
        let tip = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut bytes.as_slice())
            .unwrap()
            .0
            .unwrap()
            .0
            .expect("a best tip");
        let protocol_state = &tip.data.header.protocol_state;
        let query: GetAncestryV2Query = WithHashV1 {
            data: protocol_state.body.consensus_state.clone(),
            hash: protocol_state.try_hash().unwrap().into_inner().0,
        };
        let message = Message::Query(Query {
            tag: GetAncestryV2::NAME.into(),
            version: GetAncestryV2::VERSION,
            id: 1,
            data: NeedsLength(query),
        });
        let mut encoded = Vec::new();
        message.binprot_write(&mut encoded).unwrap();

        assert!(encoded.len() > 256, "{} bytes", encoded.len());
        assert!(
            encoded.len() <= P2pLimits::default().rpc_query(),
            "{} bytes",
            encoded.len()
        );
    }

    #[test]
    fn test_limits() {
        let limit = Limit::Some(10);
        assert!(0 < limit);
        assert!(10 <= limit);
        assert!(11 > limit);

        let unlimited = Limit::Unlimited;
        assert!(0 < unlimited);
        assert!(usize::MAX < unlimited);
    }
}

#[cfg(test)]
mod catchup_tests {
    use super::*;

    #[test]
    fn catchup_defaults_to_all_peers_with_optional_restriction() {
        let own = crate::identity::SecretKey::deterministic(1)
            .public_key()
            .peer_id();
        let other = crate::identity::SecretKey::deterministic(2)
            .public_key()
            .peer_id();
        let mut config = P2pCatchupRpcConfig::default();
        assert!(config.allows(&own));
        assert!(config.allows(&other));
        config.peers.insert(own);
        assert!(config.allows(&own));
        assert!(!config.allows(&other));
        config.enabled = false;
        assert!(!config.allows(&own));
    }
}
