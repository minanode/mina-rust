use crate::{bootstrap::P2pNetworkKadBootstrapRequestStat, P2pLimits, P2pNetworkKadEntry};
use mina_core::{debug, Substate};
use redux::ActionWithMeta;

use super::{
    bootstrap::P2pNetworkKadBootstrapState,
    request::P2pNetworkKadRequestState,
    stream::{P2pNetworkKadStreamState, P2pNetworkKademliaStreamAction},
    P2pNetworkKadAction, P2pNetworkKadBootstrapAction, P2pNetworkKadKey,
    P2pNetworkKadLatestRequestPeerKind, P2pNetworkKadRequestAction, P2pNetworkKadState,
    P2pNetworkKadStatus, P2pNetworkKademliaAction, P2pNetworkKademliaRpcReply,
};

impl super::P2pNetworkKadState {
    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, Self>,
        action: ActionWithMeta<P2pNetworkKadAction>,
        limits: &P2pLimits,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let state = state_context.get_substate_mut()?;
        let (action, meta) = action.split();
        match action {
            P2pNetworkKadAction::System(action) => P2pNetworkKadState::system_reducer(
                Substate::from_compatible_substate(state_context),
                meta.with_action(action),
            ),
            P2pNetworkKadAction::Bootstrap(action) => {
                let filter_addrs = state.filter_addrs;
                P2pNetworkKadBootstrapState::reducer(
                    Substate::from_compatible_substate(state_context),
                    meta.with_action(action),
                    filter_addrs,
                )
            }
            P2pNetworkKadAction::Request(request) => {
                P2pNetworkKadRequestState::reducer(state_context, meta.with_action(request))
            }
            P2pNetworkKadAction::Stream(action) => {
                P2pNetworkKadStreamState::reducer(state_context, meta.with_action(action), limits)
            }
        }
    }

    pub fn system_reducer<State, Action>(
        mut state_context: Substate<Action, State, Self>,
        action: ActionWithMeta<P2pNetworkKademliaAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (mut action, meta) = action.split();
        // Filter before routing insertion and before forwarding replies to
        // bootstrap. Keep alternate addresses for the same peer id.
        let p2p_context =
            Substate::<Action, State, crate::P2pState>::from_compatible_substate(state_context);
        let p2p = p2p_context.get_substate()?;
        let config = p2p.dial_backoff_config();
        let keep = |peer_id, addr: &multiaddr::Multiaddr| {
            let mut protocols = addr.iter();
            let socket = match (protocols.next(), protocols.next()) {
                (Some(multiaddr::Protocol::Ip4(ip)), Some(multiaddr::Protocol::Tcp(port))) => {
                    Some(std::net::SocketAddr::from((ip, port)))
                }
                (Some(multiaddr::Protocol::Ip6(ip)), Some(multiaddr::Protocol::Tcp(port))) => {
                    Some(std::net::SocketAddr::from((ip, port)))
                }
                _ => None,
            };
            !p2p.dial_backoff
                .is_stale(&peer_id, socket, meta.time(), &config)
        };
        match &mut action {
            P2pNetworkKademliaAction::UpdateFindNodeRequest { closest_peers, .. } => {
                for entry in closest_peers.iter_mut() {
                    let peer_id = entry.peer_id;
                    entry.retain_addresses(|addr| keep(peer_id, addr));
                }
                closest_peers.retain(|entry| !entry.addresses().is_empty());
            }
            P2pNetworkKademliaAction::UpdateRoutingTable { peer_id, addrs } => {
                addrs.retain(|addr| keep(*peer_id, addr));
                if addrs.is_empty() {
                    return Ok(());
                }
            }
            _ => {}
        }
        state_context = Substate::from_compatible_substate(p2p_context);
        let state = state_context.get_substate_mut()?;
        match (&mut state.status, action) {
            (
                _,
                P2pNetworkKademliaAction::AnswerFindNodeRequest {
                    addr,
                    peer_id,
                    stream_id,
                    key,
                },
            ) => {
                let kad_key = P2pNetworkKadKey::from(key);
                let closer_peers: Vec<_> =
                    state.routing_table.find_node(&kad_key).cloned().collect();
                debug!(meta.time(); "found {} peers", closer_peers.len());
                let message = P2pNetworkKademliaRpcReply::FindNode { closer_peers };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkKademliaStreamAction::SendResponse {
                    addr,
                    peer_id,
                    stream_id,
                    data: message,
                });
                Ok(())
            }
            (
                _,
                P2pNetworkKademliaAction::UpdateFindNodeRequest {
                    closest_peers,
                    peer_id,
                    stream_id,
                    ..
                },
            ) => {
                let mut latest_request_peers = Vec::new();
                for entry in &closest_peers {
                    let kind = match state.routing_table.insert(entry.clone()) {
                        Ok(true) => P2pNetworkKadLatestRequestPeerKind::New,
                        Ok(false) => P2pNetworkKadLatestRequestPeerKind::Existing,
                        Err(_) => P2pNetworkKadLatestRequestPeerKind::Discarded,
                    };
                    latest_request_peers.push((entry.peer_id, kind));
                }
                state.latest_request_peers = latest_request_peers.into();

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkKadRequestAction::ReplyReceived {
                    peer_id,
                    stream_id,
                    data: closest_peers,
                });

                Ok(())
            }
            (_, P2pNetworkKademliaAction::StartBootstrap { key }) => {
                let mut bootstrap_state =
                    P2pNetworkKadBootstrapState::new(key).map_err(|k| k.to_string())?;
                bootstrap_state.started_at = Some(meta.time());
                state.status = P2pNetworkKadStatus::Bootstrapping(bootstrap_state);

                if state
                    .bootstrap_state()
                    .is_some_and(|bootstrap_state| bootstrap_state.requests.len() < super::ALPHA)
                {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(P2pNetworkKadBootstrapAction::CreateRequests);
                }

                Ok(())
            }
            (
                P2pNetworkKadStatus::Bootstrapping(bootstrap_state),
                P2pNetworkKademliaAction::BootstrapFinished,
            ) => {
                let (mut successful, mut failed, mut ongoing) = (0, 0, 0);
                for request in &bootstrap_state.stats.requests {
                    match request {
                        P2pNetworkKadBootstrapRequestStat::Successful(_) => successful += 1,
                        P2pNetworkKadBootstrapRequestStat::Failed(_) => failed += 1,
                        P2pNetworkKadBootstrapRequestStat::Ongoing(_) => ongoing += 1,
                    }
                }
                mina_core::debug!(meta.time();
                    kind = "P2pKadLookupFinished",
                    summary = "Kademlia lookup finished",
                    key = display(bootstrap_state.key),
                    successful = successful,
                    failed = failed,
                    // Request records still marked ongoing; the request
                    // bookkeeping can leave some behind when it asks a peer
                    // twice, so `in_flight` is what the lookup waited for.
                    unfinished = ongoing,
                    in_flight = bootstrap_state.requests.len(),
                    routing_table_peers = state
                        .routing_table
                        .buckets
                        .iter()
                        .map(|bucket| bucket.len())
                        .sum::<usize>(),
                    took_ms = bootstrap_state
                        .started_at
                        .and_then(|t| meta.time().checked_sub(t))
                        .map_or(0, |d| d.as_millis() as u64),
                );
                state.status = P2pNetworkKadStatus::Bootstrapped {
                    time: meta.time(),
                    stats: bootstrap_state.stats.clone(),
                };
                state.has_bootstrapped = true;
                Ok(())
            }
            (_, P2pNetworkKademliaAction::UpdateRoutingTable { peer_id, addrs }) => {
                let _ = state.routing_table.insert(
                    P2pNetworkKadEntry::new(peer_id, addrs.clone()).map_err(|e| e.to_string())?,
                );
                Ok(())
            }
            (state, action) => Err(format!("invalid action {action:?} for state {state:?}")),
        }
    }
}
