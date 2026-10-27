use juniper::GraphQLObject;
use mina_core::{
    consensus::{ConsensusConstants, ConsensusTime},
    constants::ConstraintConstants,
};
use mina_node::{
    rpc::{
        ConsensusTimeQuery, PeerConnectionStatus, RpcConsensusSlot, RpcConsensusTimeGetResponse,
        RpcMetricsGetResponse, RpcNextBlockProduction, RpcNodeStatusNetworkInfo, RpcPeerInfo,
        RpcRequest,
    },
    BuildEnv,
};

use super::{Context, ConversionError, Error, SyncStatus};

/// The connected peers, as `getPeers` and `daemonStatus.peers` list them.
pub(crate) async fn fetch_connected_peers(
    context: &Context,
) -> juniper::FieldResult<Vec<GraphQLRpcPeerInfo>> {
    let peers: Vec<RpcPeerInfo> = context
        .rpc_sender
        .oneshot_request(RpcRequest::PeersGet)
        .await
        .ok_or(Error::StateMachineEmptyResponse)?;

    Ok(peers
        .iter()
        .filter(|peer| matches!(peer.connection_status, PeerConnectionStatus::Connected))
        .map(GraphQLRpcPeerInfo::from)
        .collect())
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L314-L341>
#[derive(Clone, Debug, Copy)]
pub(crate) struct GraphQLDaemonStatus;

#[juniper::graphql_object(context = Context, name = "DaemonStatus")]
impl GraphQLDaemonStatus {
    async fn sync_status(&self, context: &Context) -> juniper::FieldResult<SyncStatus> {
        super::fetch_sync_status(context).await
    }

    /// Seconds since node state initialization, as `/status` reports.
    async fn uptime_secs(&self, context: &Context) -> juniper::FieldResult<i32> {
        let status = context
            .get_or_fetch_status()
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;
        Ok(status.uptime_secs.try_into().unwrap_or(i32::MAX))
    }

    /// OCaml lists the states of its catch-up jobs. This node's catch-up
    /// works differently and has none to list.
    async fn catchup_status(&self, _context: &Context) -> Option<Vec<String>> {
        None
    }

    /// OCaml's non-null status metrics, read from the same node-state snapshot
    /// as the operator endpoint.
    async fn metrics(&self, context: &Context) -> juniper::FieldResult<GraphQLMetrics> {
        let snapshot: RpcMetricsGetResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::MetricsGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;
        let to_i32 = |n: u64| i32::try_from(n).unwrap_or(i32::MAX);
        Ok(GraphQLMetrics {
            block_production_delay: snapshot.block_production_delay_buckets.map(to_i32).to_vec(),
            transaction_pool_diff_received: to_i32(snapshot.transaction_pool_diff_received),
            transaction_pool_diff_broadcasted: to_i32(snapshot.transaction_pool_diff_broadcasted),
            transactions_added_to_pool: to_i32(snapshot.transactions_added_to_pool),
            transaction_pool_size: to_i32(snapshot.transaction_pool_size as u64),
            snark_pool_diff_received: to_i32(snapshot.snark_pool_diff_received),
            snark_pool_diff_broadcasted: to_i32(snapshot.snark_pool_diff_broadcasted),
            pending_snark_work: to_i32(
                snapshot
                    .snark_pool_jobs
                    .saturating_sub(snapshot.snark_pool_completed) as u64,
            ),
            snark_pool_size: to_i32(snapshot.snark_pool_completed as u64),
        })
    }

    async fn consensus_configuration(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLConsensusConfiguration> {
        let consensus_constants: ConsensusConstants = context
            .rpc_sender
            .oneshot_request(RpcRequest::ConsensusConstantsGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;
        Ok(GraphQLConsensusConfiguration::try_from(
            consensus_constants,
        )?)
    }

    async fn peers(&self, context: &Context) -> juniper::FieldResult<Vec<GraphQLRpcPeerInfo>> {
        fetch_connected_peers(context).await
    }

    async fn consensus_time_now(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLConsensusTime> {
        let consensus_time: RpcConsensusTimeGetResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::ConsensusTimeGet(ConsensusTimeQuery::Now))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        match consensus_time {
            Some(consensus_time) => Ok(GraphQLConsensusTime::from(consensus_time)),
            None => Err(juniper::FieldError::new(
                "No consensus time found",
                juniper::Value::Null,
            )),
        }
    }

    async fn consensus_time_best_tip(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLConsensusTime> {
        let consensus_time_res: RpcConsensusTimeGetResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::ConsensusTimeGet(ConsensusTimeQuery::BestTip))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        match consensus_time_res {
            Some(consensus_time) => Ok(GraphQLConsensusTime::from(consensus_time)),
            None => Err(juniper::FieldError::new(
                "No consensus time found",
                juniper::Value::Null,
            )),
        }
    }

    async fn consensus_mechanism(&self, _context: &Context) -> juniper::FieldResult<String> {
        Ok("proof_of_stake".to_string())
    }

    async fn blockchain_length(&self, context: &Context) -> juniper::FieldResult<Option<i32>> {
        let status = context.get_or_fetch_status().await;

        Ok(status.and_then(|status| {
            status
                .transition_frontier
                .best_tip
                .map(|block_summary| block_summary.height as i32)
        }))
    }

    async fn chain_id(&self, context: &Context) -> juniper::FieldResult<Option<String>> {
        let status = context.get_or_fetch_status().await;

        Ok(status.and_then(|status| status.chain_id))
    }

    async fn commit_id(&self, _context: &Context) -> juniper::FieldResult<String> {
        Ok(BuildEnv::get().git.commit_hash.to_string())
    }

    async fn global_slot_since_genesis_best_tip(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<Option<i32>> {
        let best_tip = context.get_or_fetch_best_tip().await;
        Ok(best_tip.and_then(|best_tip| best_tip.global_slot_since_genesis().try_into().ok()))
    }

    async fn ledger_merkle_root(&self, context: &Context) -> juniper::FieldResult<Option<String>> {
        let best_tip = context.get_or_fetch_best_tip().await;

        Ok(best_tip.map(|best_tip| best_tip.merkle_root_hash().to_string()))
    }

    async fn state_hash(&self, context: &Context) -> juniper::FieldResult<Option<String>> {
        let best_tip = context.get_or_fetch_best_tip().await;
        Ok(best_tip.map(|best_tip| best_tip.hash().to_string()))
    }

    async fn num_accounts(&self, context: &Context) -> juniper::FieldResult<Option<i32>> {
        let best_tip = context.get_or_fetch_best_tip().await;

        match best_tip {
            Some(best_tip) => {
                let ledger_status = context
                    .get_or_fetch_ledger_status(best_tip.merkle_root_hash())
                    .await;
                Ok(ledger_status.map(|ledger_status| ledger_status.num_accounts as i32))
            }
            None => Ok(None),
        }
    }

    async fn highest_unvalidated_block_length_received(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<Option<i32>> {
        let status = context.get_or_fetch_status().await;
        Ok(status.and_then(|status| {
            status
                .peers
                .iter()
                .filter(|peer| matches!(peer.connection_status, PeerConnectionStatus::Connected))
                .filter_map(|peer| peer.best_tip_height)
                .chain(
                    status
                        .transition_frontier
                        .best_tip
                        .as_ref()
                        .map(|tip| tip.height),
                )
                .chain(
                    status
                        .transition_frontier
                        .sync
                        .target
                        .as_ref()
                        .map(|tip| tip.height),
                )
                .max()
                .and_then(|height| i32::try_from(height).ok())
        }))
    }

    async fn highest_block_length_received(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<Option<i32>> {
        let status = context.get_or_fetch_status().await;
        Ok(status.and_then(|status| {
            status
                .transition_frontier
                .best_tip
                .and_then(|tip| i32::try_from(tip.height).ok())
        }))
    }

    async fn addrs_and_ports(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLAddrsAndPorts> {
        let status = context
            .get_or_fetch_status()
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;
        Ok(GraphQLAddrsAndPorts::from(&status.network_info))
    }

    async fn block_production_keys(&self, context: &Context) -> juniper::FieldResult<Vec<String>> {
        let status = context.get_or_fetch_status().await;
        Ok(status.map_or(vec![], |status| {
            status
                .block_producer
                .map_or(vec![], |key| vec![key.to_string()])
        }))
    }

    async fn coinbase_receiver(&self, context: &Context) -> juniper::FieldResult<Option<String>> {
        let status = context.get_or_fetch_status().await;
        Ok(status.and_then(|status| status.coinbase_receiver.map(|key| key.to_string())))
    }

    /// When this node's block producer produces next: null without a block
    /// producer or a best tip, else the next won slot, if the VRF evaluator
    /// has found one, and the best tip's slot it was worked out from.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L320>
    async fn next_block_production(
        &self,
        context: &Context,
    ) -> Option<GraphQLBlockProducerTimings> {
        let status = context.get_or_fetch_status().await?;
        status
            .next_block_production
            .map(GraphQLBlockProducerTimings::from)
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L145-L206>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "BlockProducerTimings")]
pub struct GraphQLBlockProducerTimings {
    /// Next block production time
    pub times: Vec<GraphQLConsensusTime>,
    /// Next block production global-slot-since-genesis
    pub global_slot_since_genesis: Vec<String>,
    /// Consensus time of the block that was used to determine the next block
    /// production time
    pub generated_from_consensus_at: GraphQLConsensusTimeGlobalSlot,
}

/// Consensus time and the corresponding global slot since genesis
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L129-L143>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "ConsensusTimeGlobalSlot")]
pub struct GraphQLConsensusTimeGlobalSlot {
    /// Time in terms of slot number in an epoch, start and end time of the
    /// slot since UTC epoch
    pub consensus_time: GraphQLConsensusTime,
    pub global_slot_since_genesis: String,
}

impl From<RpcNextBlockProduction> for GraphQLBlockProducerTimings {
    fn from(timing: RpcNextBlockProduction) -> Self {
        let won = timing.next_won_slot;
        Self {
            global_slot_since_genesis: won
                .iter()
                .map(|slot| slot.global_slot_since_genesis.to_string())
                .collect(),
            times: won
                .as_ref()
                .map(GraphQLConsensusTime::from)
                .into_iter()
                .collect(),
            generated_from_consensus_at: GraphQLConsensusTimeGlobalSlot {
                global_slot_since_genesis: timing
                    .generated_from_consensus_at
                    .global_slot_since_genesis
                    .to_string(),
                consensus_time: GraphQLConsensusTime::from(&timing.generated_from_consensus_at),
            },
        }
    }
}

impl From<&RpcConsensusSlot> for GraphQLConsensusTime {
    fn from(slot: &RpcConsensusSlot) -> Self {
        GraphQLConsensusTime {
            start_time: slot.start_time_ms.to_string(),
            end_time: slot.end_time_ms.to_string(),
            epoch: slot.epoch.to_string(),
            global_slot: slot.global_slot.to_string(),
            slot: slot.slot.to_string(),
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L301-L311>
#[derive(GraphQLObject, Clone, Debug)]
#[graphql(name = "Metrics")]
pub struct GraphQLMetrics {
    pub block_production_delay: Vec<i32>,
    pub transaction_pool_diff_received: i32,
    pub transaction_pool_diff_broadcasted: i32,
    pub transactions_added_to_pool: i32,
    pub transaction_pool_size: i32,
    pub snark_pool_diff_received: i32,
    pub snark_pool_diff_broadcasted: i32,
    pub pending_snark_work: i32,
    pub snark_pool_size: i32,
}

#[derive(GraphQLObject, Clone, Debug)]
#[graphql(name = "AddrsAndPorts")]
pub struct GraphQLAddrsAndPorts {
    pub bind_ip: String,
    pub external_ip: Option<String>,
    pub client_port: Option<i32>,
    pub libp2p_port: Option<i32>,
    /// This node as a peer. Not known to the GraphQL server yet, so null.
    pub peer: Option<GraphQLRpcPeerInfo>,
}

impl Default for GraphQLAddrsAndPorts {
    fn default() -> Self {
        Self {
            bind_ip: "0.0.0.0".to_string(),
            external_ip: None,
            client_port: None,
            libp2p_port: None,
            peer: None,
        }
    }
}

impl From<&RpcNodeStatusNetworkInfo> for GraphQLAddrsAndPorts {
    fn from(network_info: &RpcNodeStatusNetworkInfo) -> Self {
        Self {
            bind_ip: network_info.bind_ip.clone(),
            external_ip: network_info.external_ip.clone(),
            client_port: network_info.client_port.map(|port| port.into()),
            libp2p_port: network_info.libp2p_port.map(|port| port.into()),
            peer: None,
        }
    }
}

/// A connected peer: OCaml's `Peer` fields (`peerId`, `host`, `libp2pPort`)
/// and this node's own.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L286-L291>
#[derive(GraphQLObject, Clone, Debug)]
#[graphql(name = "Peer")]
pub struct GraphQLRpcPeerInfo {
    pub peer_id: String,
    /// The host of the peer's address: an IP address or a DNS name, or empty
    /// when its address has none
    pub host: String,
    /// The TCP port of the peer's address, or 0 when its address has none
    pub libp2p_port: i32,
    pub best_tip: Option<String>,
    pub best_tip_height: Option<String>,
    pub best_tip_global_slot: Option<String>,
    pub best_tip_timestamp: Option<String>,
    pub connection_status: String,
    pub connecting_details: Option<String>,
    pub address: Option<String>,
    pub incoming: bool,
    pub is_libp2p: bool,
    pub time: String,
}

/// The host and TCP port of a multiaddr such as `/ip4/1.2.3.4/tcp/8302/p2p/...`.
pub(crate) fn multiaddr_host_and_port(address: &str) -> (String, i32) {
    let parts: Vec<&str> = address.split('/').collect();
    let after = |keys: &[&str]| {
        parts
            .windows(2)
            .find(|w| keys.contains(&w[0]))
            .map(|w| w[1].to_string())
    };
    let host = after(&["ip4", "ip6", "dns", "dns4", "dns6"]).unwrap_or_default();
    let port = after(&["tcp"])
        .and_then(|port| port.parse().ok())
        .unwrap_or(0);
    (host, port)
}

impl From<&RpcPeerInfo> for GraphQLRpcPeerInfo {
    fn from(peer: &RpcPeerInfo) -> Self {
        let (host, libp2p_port) = peer
            .address
            .as_deref()
            .map(multiaddr_host_and_port)
            .unwrap_or_default();
        Self {
            peer_id: peer.peer_id.to_string(),
            host,
            libp2p_port,
            best_tip: peer.best_tip.as_ref().map(|hash| hash.to_string()),
            best_tip_height: peer.best_tip_height.map(|height| height.to_string()),
            best_tip_global_slot: peer.best_tip_global_slot.map(|slot| slot.to_string()),
            best_tip_timestamp: peer
                .best_tip_timestamp
                .map(|timestamp| timestamp.to_string()),
            connection_status: peer.connection_status.to_string(),
            connecting_details: peer.connecting_details.clone(),
            address: peer.address.clone(),
            incoming: peer.incoming,
            is_libp2p: peer.is_libp2p,
            time: peer.time.to_string(),
        }
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLGenesisConstants {
    pub genesis_timestamp: String,
    pub coinbase: String,
    pub account_creation_fee: String,
}

impl GraphQLGenesisConstants {
    pub fn try_new(
        constrain_constants: ConstraintConstants,
        consensus_constants: ConsensusConstants,
    ) -> Result<Self, ConversionError> {
        Ok(GraphQLGenesisConstants {
            genesis_timestamp: genesis_timestamp_to_string(
                consensus_constants.genesis_state_timestamp.as_u64(),
            )?,
            coinbase: constrain_constants.coinbase_amount.to_string(),
            account_creation_fee: constrain_constants.account_creation_fee.to_string(),
        })
    }
}

/// The genesis timestamp, in milliseconds since the epoch, as OCaml writes
/// it: ISO 8601 at UTC-08:00, with microseconds.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_constants/genesis_constants.ml#L127-L129>
pub(crate) fn genesis_timestamp_to_string(ms: u64) -> Result<String, ConversionError> {
    let format = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6][offset_hour sign:mandatory]:[offset_minute]"
    );
    let offset =
        time::UtcOffset::from_hms(-8, 0, 0).map_err(|e| ConversionError::Custom(e.to_string()))?;
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|e| ConversionError::Custom(e.to_string()))?
        .to_offset(offset)
        .format(&format)
        .map_err(|e| ConversionError::Custom(e.to_string()))
}

/// Durations are in milliseconds. OCaml writes `genesisStateTimestamp` with
/// `Core.Time.to_string` in the daemon's local time zone; use UTC consistently.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L277-L284>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "ConsensusConfiguration")]
pub struct GraphQLConsensusConfiguration {
    pub delta: i32,
    pub epoch_duration: i32,
    pub k: i32,
    pub slot_duration: i32,
    pub slots_per_epoch: i32,
    pub acceptable_network_delay: i32,
    pub genesis_state_timestamp: String,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L115-L128>
impl TryFrom<ConsensusConstants> for GraphQLConsensusConfiguration {
    type Error = ConversionError;

    fn try_from(consensus_constants: ConsensusConstants) -> Result<Self, Self::Error> {
        Ok(GraphQLConsensusConfiguration {
            delta: consensus_constants.delta as i32,
            epoch_duration: consensus_constants.epoch_duration as i32,
            k: consensus_constants.k as i32,
            slot_duration: consensus_constants.slot_duration_ms as i32,
            slots_per_epoch: consensus_constants.slots_per_epoch as i32,
            acceptable_network_delay: consensus_constants.delta_duration as i32,
            genesis_state_timestamp: consensus_state_timestamp_to_string(
                consensus_constants.genesis_state_timestamp.as_u64(),
            )?,
        })
    }
}

/// `Core.Time.to_string` for a daemon in UTC: a space separator and six
/// fractional digits, with `Z` for the UTC offset.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L277-L284>
pub(crate) fn consensus_state_timestamp_to_string(ms: u64) -> Result<String, ConversionError> {
    let format = time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]Z"
    );
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|e| ConversionError::Custom(e.to_string()))?
        .format(&format)
        .map_err(|e| ConversionError::Custom(e.to_string()))
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLConsensusTime {
    pub start_time: String,
    pub end_time: String,
    pub epoch: String,
    pub global_slot: String,
    pub slot: String,
}

impl From<ConsensusTime> for GraphQLConsensusTime {
    fn from(consensus_time: ConsensusTime) -> Self {
        let start_time: u64 = consensus_time.start_time.into();
        let end_time: u64 = consensus_time.end_time.into();

        let start_time_ms = start_time.checked_div(1_000_000).expect("division by zero");
        let end_time_ms = end_time.checked_div(1_000_000).expect("division by zero");

        GraphQLConsensusTime {
            start_time: start_time_ms.to_string(),
            end_time: end_time_ms.to_string(),
            epoch: consensus_time.epoch.to_string(),
            global_slot: consensus_time.global_slot.to_string(),
            slot: consensus_time.slot.to_string(),
        }
    }
}
