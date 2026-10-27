use std::collections::{BTreeMap, BTreeSet};

use identify::P2pNetworkIdentifyStreamAction;
use mina_core::{bug_condition, error, Substate};
use redux::Dispatcher;
use request::{P2pNetworkKadRequestState, P2pNetworkKadRequestStatus};
use token::{
    AuthKind, DiscoveryAlgorithm, IdentifyAlgorithm, MuxKind, PingAlgorithm, Protocol,
    RpcAlgorithm, StreamKind,
};

use crate::{
    connection::{
        incoming::P2pConnectionIncomingAction, outgoing::P2pConnectionOutgoingAction,
        P2pConnectionState,
    },
    disconnection::P2pDisconnectionAction,
    identify::P2pIdentifyAction,
    P2pConfig, P2pPeerAction, P2pPeerStatus, P2pState, PeerId,
};

use super::{super::*, p2p_network_scheduler_state::P2pNetworkConnectionState, *};

impl P2pNetworkSchedulerState {
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, Self>,
        action: redux::ActionWithMeta<P2pNetworkSchedulerAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let scheduler_state = state_context.get_substate_mut()?;

        match action {
            P2pNetworkSchedulerAction::InterfaceDetected { ip, .. } => {
                scheduler_state.interfaces.insert(ip);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_config: &P2pConfig = state.substate()?;

                if let Some(port) = p2p_config.libp2p_port {
                    dispatcher
                        .push(P2pNetworkSchedulerEffectfulAction::InterfaceDetected { ip, port });
                }

                Ok(())
            }
            P2pNetworkSchedulerAction::InterfaceExpired { ip, .. } => {
                scheduler_state.interfaces.remove(&ip);
                Ok(())
            }
            P2pNetworkSchedulerAction::ListenerReady { listener } => {
                scheduler_state.listeners.insert(listener);
                Ok(())
            }
            P2pNetworkSchedulerAction::ListenerError { listener, .. } => {
                scheduler_state.listeners.remove(&listener);
                Ok(())
            }
            P2pNetworkSchedulerAction::IncomingDataIsReady { addr } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let scheduler: &Self = state.substate()?;
                let Some(connection_state) = scheduler.connection_state(&addr) else {
                    bug_condition!(
                        "Invalid state for `P2pNetworkSchedulerAction::IncomingDataIsReady`"
                    );
                    return Ok(());
                };

                let limit = connection_state.limit();
                if limit > 0 {
                    dispatcher.push(P2pNetworkSchedulerEffectfulAction::IncomingDataIsReady {
                        addr,
                        limit,
                    });
                }

                Ok(())
            }
            P2pNetworkSchedulerAction::IncomingDidAccept { addr, result } => {
                if let Some(addr) = addr {
                    scheduler_state.connections.insert(
                        addr,
                        P2pNetworkConnectionState {
                            incoming: true,
                            pnet: P2pNetworkPnetState::new(scheduler_state.pnet_key, meta.time()),
                            select_auth: P2pNetworkSelectState::default(),
                            auth: None,
                            select_mux: P2pNetworkSelectState::default(),
                            mux: None,
                            streams: BTreeMap::default(),
                            closed: None,
                            limit: P2pNetworkConnectionState::INITIAL_LIMIT,
                            pubsub_reopens: None,
                        },
                    );
                };

                let dispatcher = state_context.into_dispatcher();
                if let Some(addr) = addr {
                    dispatcher.push(P2pNetworkSchedulerEffectfulAction::IncomingDidAccept {
                        addr,
                        result,
                    });
                }

                Ok(())
            }
            P2pNetworkSchedulerAction::OutgoingConnect { addr } => {
                scheduler_state.connections.insert(
                    ConnectionAddr {
                        sock_addr: addr,
                        incoming: false,
                    },
                    P2pNetworkConnectionState {
                        incoming: false,
                        pnet: P2pNetworkPnetState::new(scheduler_state.pnet_key, meta.time()),
                        select_auth: P2pNetworkSelectState::initiator_auth(
                            token::AuthKind::Noise,
                            meta.time(),
                        ),
                        auth: None,
                        select_mux: P2pNetworkSelectState::initiator_mux(
                            token::MuxKind::Yamux1_0_0,
                            meta.time(),
                        ),
                        mux: None,
                        streams: BTreeMap::default(),
                        closed: None,
                        limit: P2pNetworkConnectionState::INITIAL_LIMIT,
                        pubsub_reopens: None,
                    },
                );

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkSchedulerEffectfulAction::OutgoingConnect { addr });
                Ok(())
            }
            P2pNetworkSchedulerAction::OutgoingDidConnect { addr, result } => {
                // TODO: change to connected

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                match result {
                    Ok(()) => {
                        dispatcher
                            .push(P2pNetworkSchedulerEffectfulAction::OutgoingDidConnect { addr });
                    }
                    Err(error) => {
                        let Some((peer_id, peer_state)) = p2p_state.peer_with_connection(addr)
                        else {
                            bug_condition!(
                                "outgoing connection to {addr} failed, but there is no peer for it"
                            );
                            return Ok(());
                        };
                        if matches!(
                            peer_state.status,
                            P2pPeerStatus::Connecting(P2pConnectionState::Outgoing(_))
                        ) {
                            dispatcher.push(P2pConnectionOutgoingAction::FinalizeError {
                                peer_id,
                                error: error.to_string(),
                            });
                        } else {
                            bug_condition!("Invalid status for `P2pNetworkSchedulerAction::OutgoingDidConnect`: {:?}", peer_state.status);
                        }
                    }
                }
                Ok(())
            }
            P2pNetworkSchedulerAction::IncomingDataDidReceive { result, addr } => {
                // since both actions dispatcher later require connection state, if we can't find it we shouldn't dispatcher them
                let Some(state) = scheduler_state.connection_state_mut(&addr) else {
                    bug_condition!("Unable to find connection for `P2pNetworkSchedulerAction::IncomingDataDidReceive`");
                    return Ok(());
                };

                if let Ok(data) = &result {
                    state.consume(data.len());
                };

                let dispatcher = state_context.into_dispatcher();
                match result {
                    Ok(data) => {
                        dispatcher.push(P2pNetworkPnetAction::IncomingData { addr, data });
                    }
                    Err(error) => dispatcher.push(P2pNetworkSchedulerAction::Error {
                        addr,
                        error: P2pNetworkConnectionError::MioError(error),
                    }),
                }
                Ok(())
            }
            P2pNetworkSchedulerAction::SelectDone {
                addr,
                kind,
                protocol,
                incoming,
                expected_peer_id,
            } => {
                scheduler_state.reducer_select_done(
                    addr,
                    kind,
                    protocol,
                    incoming,
                    expected_peer_id,
                );

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;
                Self::forward_select_done(dispatcher, p2p_state, protocol, addr, incoming, kind);
                Ok(())
            }
            P2pNetworkSchedulerAction::SelectError { addr, kind, error } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let scheduler: &Self = state.substate()?;
                let Some(connection) = scheduler.connection_state(&addr) else {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::SelectError`: no connection {addr}"
                    );
                    return Ok(());
                };

                match kind {
                    // A stream that failed to negotiate is reset on its own, as
                    // go-libp2p's responder does, unless the connection needs it.
                    SelectKind::Stream(peer_id, stream_id)
                        if !connection.is_essential_stream(&addr, stream_id) =>
                    {
                        dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                            addr,
                            stream_id,
                            data: Data::default(),
                            flags: YamuxFlags::RST,
                        });
                        dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                            addr,
                            peer_id,
                            stream_id,
                            protocol: None,
                            reason: P2pNetworkStreamResetReason::SelectError(error),
                        });
                    }
                    _ => {
                        dispatcher.push(P2pNetworkSchedulerAction::Error {
                            addr,
                            error: P2pNetworkConnectionError::SelectError,
                        });
                    }
                }

                Ok(())
            }
            P2pNetworkSchedulerAction::YamuxDidInit {
                addr,
                peer_id,
                message_size_limit,
                pending_outgoing_limit,
            } => {
                let Some(cn) = scheduler_state.connections.get_mut(&addr) else {
                    bug_condition!(
                        "Missing connection state for `P2pNetworkSchedulerAction::YamuxDidInit`"
                    );
                    return Ok(());
                };
                if let Some(P2pNetworkConnectionMuxState::Yamux(yamux)) = &mut cn.mux {
                    yamux.init = true;
                    yamux.message_size_limit = message_size_limit;
                    yamux.pending_outgoing_limit = pending_outgoing_limit;
                }

                let incoming = cn.incoming;
                let dispatcher = state_context.into_dispatcher();

                if incoming {
                    dispatcher.push(P2pConnectionIncomingAction::Libp2pReceived { peer_id });
                } else {
                    dispatcher.push(P2pConnectionOutgoingAction::FinalizeSuccess {
                        peer_id,
                        remote_auth: None,
                    });
                }

                dispatcher.push(P2pIdentifyAction::NewRequest { peer_id, addr });
                Ok(())
            }
            P2pNetworkSchedulerAction::Disconnect { addr, reason } => {
                let Some(conn_state) = scheduler_state.connections.get_mut(&addr) else {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::Disconnect`: connection {addr} does not exist"
                    );
                    return Ok(());
                };
                if conn_state.closed.is_some() {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::Disconnect`: {addr} already disconnected"
                    );
                    return Ok(());
                }
                conn_state.closed = Some(reason.clone().into());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkSchedulerEffectfulAction::Disconnect {
                    addr,
                    reason: P2pNetworkConnectionCloseReason::Disconnect(reason),
                });

                Ok(())
            }
            P2pNetworkSchedulerAction::Error { addr, error } => {
                let Some(conn_state) = scheduler_state.connections.get_mut(&addr) else {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::Error`: connection {addr} does not exist"
                    );
                    return Ok(());
                };
                if conn_state.closed.is_some() {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::Error`: {addr} already disconnected"
                    );
                    return Ok(());
                }
                conn_state.closed = Some(error.clone().into());

                let dispatcher = state_context.into_dispatcher();
                // The address we dialled answered as another peer id: the
                // record is stale (I18).
                if let P2pNetworkConnectionError::Noise(NoiseError::RemotePeerIdMismatch {
                    expected,
                    got,
                }) = &error
                {
                    if !addr.incoming {
                        dispatcher.push(P2pPeerAction::Stale {
                            peer_id: *expected,
                            addr: addr.sock_addr,
                            answered_by: *got,
                        });
                    }
                }
                dispatcher.push(P2pNetworkSchedulerEffectfulAction::Disconnect {
                    addr,
                    reason: P2pNetworkConnectionCloseReason::Error(error),
                });
                Ok(())
            }
            P2pNetworkSchedulerAction::Disconnected { addr, reason } => {
                let Some(cn) = scheduler_state.connections.get_mut(&addr) else {
                    bug_condition!(
                        "P2pNetworkSchedulerAction::Disconnected: connection {addr} does not exist"
                    );
                    return Ok(());
                };
                if cn.closed.is_none() {
                    bug_condition!(
                        "P2pNetworkSchedulerAction::Disconnect: {addr} is not disconnected"
                    );
                }

                let incoming = cn.incoming;
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let state: &P2pState = state.substate()?;

                let peer_with_state = state.peer_with_connection(addr);

                if reason.is_disconnected() {
                    // statemachine behaviour should continue with this, i.e. dispatch P2pDisconnectionAction::Finish
                    return Ok(());
                }

                match peer_with_state {
                    Some((peer_id, peer_state)) => {
                        // TODO: connection state type should tell if it is finalized
                        match &peer_state.status {
                            P2pPeerStatus::Connecting(
                                crate::connection::P2pConnectionState::Incoming(_),
                            ) => {
                                dispatcher.push(P2pConnectionIncomingAction::FinalizeError {
                                    peer_id,
                                    error: reason.to_string(),
                                });
                            }
                            P2pPeerStatus::Connecting(
                                crate::connection::P2pConnectionState::Outgoing(_),
                            ) => {
                                dispatcher.push(P2pConnectionOutgoingAction::FinalizeError {
                                    peer_id,
                                    error: reason.to_string(),
                                });
                            }
                            P2pPeerStatus::Disconnecting { .. } => {}
                            P2pPeerStatus::Disconnected { .. } => {
                                // sanity check, should be incoming connection
                                if !incoming {
                                    error!(meta.time(); "disconnected peer connection for address {addr}");
                                } else {
                                    // TODO: introduce action for incoming connection finalization without peer_id
                                }
                            }
                            P2pPeerStatus::Ready(_) => {
                                dispatcher.push(P2pDisconnectionAction::Finish { peer_id });
                            }
                        }
                    }
                    None => {
                        // sanity check, should be incoming connection
                        if !incoming {
                            // TODO: error!(meta.time(); "non-existing peer connection for address {addr}");
                        } else {
                            // TODO: introduce action for incoming connection finalization without peer_id
                        }
                    }
                }
                Ok(())
            }
            P2pNetworkSchedulerAction::Prune { addr } => {
                if let Some(old) = scheduler_state.connections.remove(&addr) {
                    if let Some(peer_id) = old.peer_id() {
                        scheduler_state.prune_peer_state(peer_id);
                    }
                }
                Ok(())
            }
            P2pNetworkSchedulerAction::StreamReset {
                addr,
                peer_id,
                stream_id,
                protocol,
                ..
            } => {
                let Some(connection) = scheduler_state.connections.get_mut(&addr) else {
                    bug_condition!(
                        "`P2pNetworkSchedulerAction::StreamReset`: no connection {addr}"
                    );
                    return Ok(());
                };
                connection.remove_stream(stream_id);

                let opened_by_peer =
                    P2pNetworkConnectionState::stream_opened_by_peer(&addr, stream_id);
                // The peer reset our pubsub stream, and the connection stays up:
                // open it again, or the peer gets none of our gossip.
                let reopen_pubsub = match protocol {
                    Some(kind @ token::StreamKind::Broadcast(_)) if !opened_by_peer => {
                        connection.may_reopen_pubsub(meta.time()).then_some(kind)
                    }
                    _ => None,
                };
                // Only a negotiated pubsub stream can have left a partial
                // message in the peer's buffer.
                let pubsub_incoming =
                    opened_by_peer && matches!(protocol, Some(token::StreamKind::Broadcast(_)));
                let was_incoming_rpc = scheduler_state
                    .rpc_incoming_streams
                    .get(&peer_id)
                    .is_some_and(|streams| streams.contains_key(&stream_id));
                scheduler_state.prune_stream_state(&peer_id, stream_id, pubsub_incoming);

                // Our Kademlia request to this peer can't complete without its
                // stream: fail it, so that the bootstrap doesn't wait for it.
                let kad_request_waits = !opened_by_peer
                    && stream_id == YamuxStreamKind::Kademlia.stream_id(addr.incoming)
                    && scheduler_state
                        .discovery_state()
                        .and_then(|discovery| discovery.request(&peer_id))
                        .is_some_and(|request| {
                            matches!(
                                request.status,
                                P2pNetworkKadRequestStatus::WaitingForKadStream(_)
                                    | P2pNetworkKadRequestStatus::Request(_)
                                    | P2pNetworkKadRequestStatus::WaitingForReply
                            )
                        });

                // A dead stream cannot receive the async ledger results still
                // being computed for it. Release its channel budget too. Use
                // the surviving correlations, including legacy wire IDs, so a
                // close cannot cancel another stream's request with the same ID.
                let live_rpc_ids: BTreeSet<_> = scheduler_state
                    .rpc_incoming_streams
                    .get(&peer_id)
                    .into_iter()
                    .flat_map(|streams| streams.values())
                    .flat_map(|rpc| {
                        rpc.pending_incoming
                            .keys()
                            .copied()
                            .chain(rpc.pending.as_ref().map(|query| query.id))
                    })
                    .collect();
                let mut state_context: Substate<Action, State, P2pState> =
                    Substate::from_compatible_substate(state_context);
                if was_incoming_rpc {
                    if let Some(peer) = state_context
                        .get_substate_mut()?
                        .get_ready_peer_mut(&peer_id)
                    {
                        if let crate::channels::rpc::P2pChannelsRpcState::Ready { remote, .. } =
                            &mut peer.channels.rpc
                        {
                            remote
                                .pending_requests
                                .retain(|request| live_rpc_ids.contains(&request.id));
                        }
                    }
                }
                let dispatcher = state_context.into_dispatcher();
                if kad_request_waits {
                    dispatcher.push(P2pNetworkKadRequestAction::Error {
                        peer_id,
                        error: format!("stream {stream_id} was reset"),
                    });
                }
                // The same id is free again: the peer forgot the stream when it
                // reset it (go-yamux `Stream.Reset`, `cleanup`).
                if let Some(stream_kind) = reopen_pubsub {
                    mina_core::debug!(meta.time();
                        summary = "reopening our pubsub stream after the peer reset it",
                        addr = display(addr),
                        peer_id = display(peer_id),
                        stream_id = stream_id
                    );
                    dispatcher.push(P2pNetworkYamuxAction::OpenStream {
                        addr,
                        stream_id,
                        stream_kind,
                    });
                }

                Ok(())
            }
            P2pNetworkSchedulerAction::IncomingConnectionIsReady { listener } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                let should_accept = p2p_state.network.scheduler.connections.len()
                    < p2p_state.config.limits.max_connections();

                dispatcher.push(
                    P2pNetworkSchedulerEffectfulAction::IncomingConnectionIsReady {
                        listener,
                        should_accept,
                    },
                );
                Ok(())
            }
        }
    }

    fn reducer_select_done(
        &mut self,
        addr: ConnectionAddr,
        kind: SelectKind,
        protocol: Option<Protocol>,
        incoming: bool,
        expected_peer_id: Option<PeerId>,
    ) {
        let Some(connection) = self.connections.get_mut(&addr) else {
            bug_condition!("Missing connection state for `P2pNetworkSchedulerAction::SelectDone`");
            return;
        };

        match protocol {
            Some(token::Protocol::Auth(token::AuthKind::Noise)) => {
                connection.auth = Some(P2pNetworkAuthState::Noise(P2pNetworkNoiseState::new(
                    self.local_pk.clone(),
                    expected_peer_id,
                )));
            }
            Some(token::Protocol::Mux(
                token::MuxKind::Yamux1_0_0 | token::MuxKind::YamuxNoNewLine1_0_0,
            )) => {
                connection.mux = Some(P2pNetworkConnectionMuxState::Yamux(P2pNetworkYamuxState {
                    init: true,
                    ..Default::default()
                }));
            }
            Some(token::Protocol::Stream(stream_kind)) => {
                let SelectKind::Stream(peer_id, stream_id) = kind else {
                    bug_condition!(
                        "incorrect stream kind {kind:?} for protocol stream: {stream_kind:?}"
                    );
                    return;
                };
                match stream_kind {
                    token::StreamKind::Rpc(_) => {
                        if incoming {
                            self.rpc_incoming_streams
                                .entry(peer_id)
                                .or_default()
                                .insert(stream_id, P2pNetworkRpcState::new(addr, stream_id));
                        } else {
                            self.rpc_outgoing_streams
                                .entry(peer_id)
                                .or_default()
                                .insert(stream_id, P2pNetworkRpcState::new(addr, stream_id));
                        }
                    }
                    token::StreamKind::Broadcast(_) => {}
                    token::StreamKind::Identify(_) => {}
                    token::StreamKind::Discovery(_) => {}
                    token::StreamKind::Ping(_) => {}
                    token::StreamKind::Bitswap(_) => {}
                    token::StreamKind::Status(_) => {}
                }
            }
            None => {}
        }
    }

    fn forward_select_done<Action, State>(
        dispatcher: &mut Dispatcher<Action, State>,
        p2p_state: &P2pState,
        protocol: Option<Protocol>,
        addr: ConnectionAddr,
        incoming: bool,
        select_kind: SelectKind,
    ) where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        match protocol {
            Some(Protocol::Auth(AuthKind::Noise)) => {
                dispatcher
                    .push(P2pNetworkSchedulerEffectfulAction::NoiseSelectDone { addr, incoming });
            }
            Some(Protocol::Mux(MuxKind::Yamux1_0_0 | MuxKind::YamuxNoNewLine1_0_0)) => {
                let SelectKind::Multiplexing(peer_id) = select_kind else {
                    bug_condition!("wrong kind for multiplexing protocol action: {select_kind:?}");
                    return;
                };
                let message_size_limit = p2p_state.config.limits.yamux_message_size();
                let pending_outgoing_limit =
                    p2p_state.config.limits.yamux_pending_outgoing_per_peer();
                dispatcher.push(P2pNetworkSchedulerAction::YamuxDidInit {
                    addr,
                    peer_id,
                    message_size_limit,
                    pending_outgoing_limit,
                });
            }
            Some(Protocol::Stream(kind)) => {
                let SelectKind::Stream(peer_id, stream_id) = select_kind else {
                    bug_condition!("wrong kind for stream protocol action: {kind:?}");
                    return;
                };
                match kind {
                    StreamKind::Status(_)
                    | StreamKind::Identify(IdentifyAlgorithm::IdentifyPush1_0_0)
                    | StreamKind::Bitswap(_)
                    | StreamKind::Ping(PingAlgorithm::Ping1_0_0) => {
                        //unimplemented!()
                    }
                    StreamKind::Identify(IdentifyAlgorithm::Identify1_0_0) => {
                        dispatcher.push(P2pNetworkIdentifyStreamAction::New {
                            addr,
                            peer_id,
                            stream_id,
                            incoming,
                        });
                    }

                    StreamKind::Broadcast(protocol) => {
                        dispatcher.push(P2pNetworkPubsubAction::NewStream {
                            incoming,
                            peer_id,
                            addr,
                            stream_id,
                            protocol,
                        });
                    }
                    StreamKind::Discovery(DiscoveryAlgorithm::Kademlia1_0_0) => {
                        if let Some(discovery_state) = p2p_state.network.scheduler.discovery_state()
                        {
                            let request = !incoming && discovery_state.request(&peer_id).is_some();
                            dispatcher.push(P2pNetworkKademliaStreamAction::New {
                                addr,
                                peer_id,
                                stream_id,
                                incoming,
                            });
                            // if our node initiated a request to the peer, notify that the stream is ready.
                            if request {
                                dispatcher.push(P2pNetworkKadRequestAction::StreamReady {
                                    peer_id,
                                    addr,
                                    stream_id,
                                    callback: redux::callback!(
                                        on_p2p_network_kad_request_stream_ready((
                                            addr: ConnectionAddr,
                                            peer_id: PeerId,
                                            stream_id: StreamId,
                                            data: P2pNetworkKademliaRpcRequest
                                        )) -> crate::P2pAction{
                                            P2pNetworkKademliaStreamAction::SendRequest {
                                                addr,
                                                peer_id,
                                                stream_id,
                                                data
                                            }
                                        }
                                    ),
                                });
                            }
                        }
                    }
                    StreamKind::Rpc(RpcAlgorithm::Rpc0_0_1) => {
                        dispatcher.push(P2pNetworkRpcAction::Init {
                            addr,
                            peer_id,
                            stream_id,
                            incoming,
                        });
                    }
                }
            }
            None => {
                match &select_kind {
                    SelectKind::Authentication => {
                        // TODO: close the connection
                    }
                    SelectKind::MultiplexingNoPeerId => {
                        bug_condition!("`SelectKind::MultiplexingNoPeerId` not handled");
                        // WARNING: must not happen
                    }
                    SelectKind::Multiplexing(_) => {
                        // TODO: close the connection
                    }
                    SelectKind::Stream(peer_id, stream_id) => {
                        if let Some(discovery_state) = p2p_state.network.scheduler.discovery_state()
                        {
                            if let Some(P2pNetworkKadRequestState {
                                status: P2pNetworkKadRequestStatus::WaitingForKadStream(id),
                                ..
                            }) = discovery_state.request(peer_id)
                            {
                                if id == stream_id {
                                    dispatcher.push(P2pNetworkKadRequestAction::Error {
                                        peer_id: *peer_id,
                                        error: "stream protocol is not negotiated".into(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
