use crate::{
    channels::{
        rpc::P2pChannelsRpcAction, signaling::discovery::P2pChannelsSignalingDiscoveryAction,
        streaming_rpc::P2pChannelsStreamingRpcAction, P2pChannelsState,
    },
    connection::{
        incoming::P2pConnectionIncomingAction, outgoing::P2pConnectionOutgoingAction,
        P2pConnectionState,
    },
    disconnection::{P2pDisconnectedState, P2pDisconnectionAction},
    Data, P2pAction, P2pNetworkConnectionMuxState, P2pNetworkKadKey, P2pNetworkKademliaAction,
    P2pNetworkKademliaStreamAction, P2pNetworkPnetAction, P2pNetworkPubsubAction,
    P2pNetworkRpcAction, P2pNetworkSchedulerAction, P2pNetworkSelectAction, P2pNetworkState,
    P2pNetworkStreamResetReason, P2pNetworkYamuxAction, P2pPeerState, P2pState, YamuxFlags,
};
use mina_core::{bug_condition, Substate};
use redux::{ActionMeta, ActionWithMeta, Dispatcher, Timestamp};

impl P2pState {
    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, Self>,
        action: ActionWithMeta<P2pAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let Ok(state) = state_context.get_substate_mut() else {
            bug_condition!("no P2pState");
            return Ok(());
        };
        let (action, meta) = action.split();

        match action {
            P2pAction::Initialization(_) => {
                // noop
                Ok(())
            }
            P2pAction::Connection(action) => {
                P2pConnectionState::reducer(state_context, meta.with_action(action))
            }
            P2pAction::Disconnection(action) => {
                P2pDisconnectedState::reducer(state_context, meta.with_action(action))
            }
            P2pAction::Peer(action) => P2pPeerState::reducer(
                Substate::from_compatible_substate(state_context),
                meta.with_action(action),
            ),
            P2pAction::Channels(action) => {
                P2pChannelsState::reducer(state_context, meta.with_action(action))
            }
            P2pAction::Identify(_action) => {
                #[cfg(feature = "p2p-libp2p")]
                Self::identify_reducer(state_context, meta.with_action(_action))?;
                Ok(())
            }
            P2pAction::Network(_action) => {
                #[cfg(feature = "p2p-libp2p")]
                {
                    let limits = state.config.limits;
                    let catchup_enabled = match &_action {
                        crate::P2pNetworkAction::Rpc(a) => {
                            state.is_libp2p_peer(a.peer_id())
                                && state.config.catchup_rpc.allows(a.peer_id())
                        }
                        _ => false,
                    };
                    let rpc_channel_available = match &_action {
                        crate::P2pNetworkAction::Rpc(P2pNetworkRpcAction::IncomingMessage {
                            peer_id,
                            message: crate::RpcMessage::Query { .. },
                            ..
                        }) => state.get_ready_peer(peer_id).is_some_and(|peer| {
                            matches!(&peer.channels.rpc,
                            crate::channels::rpc::P2pChannelsRpcState::Ready { remote, .. }
                                if remote.pending_requests.len()
                                    < if catchup_enabled {
                                        crate::channels::rpc::MAX_CATCHUP_RPC_REQUESTS
                                    } else {
                                        crate::channels::rpc::MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS
                                    })
                        }),
                        _ => true,
                    };
                    P2pNetworkState::reducer(
                        Substate::from_compatible_substate(state_context),
                        meta.with_action(_action),
                        &limits,
                        catchup_enabled,
                        rpc_channel_available,
                    )?;
                }
                Ok(())
            }
        }
    }

    pub fn p2p_timeout_dispatch<State, Action>(
        mut state_context: Substate<Action, State, Self>,
        meta: &ActionMeta,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let time = meta.time();
        state_context
            .get_substate_mut()?
            .network
            .refusal_warnings
            .flush(time);
        state_context
            .get_substate_mut()?
            .restore_seed_dial_opts(time);
        let (dispatcher, state) = state_context.into_dispatcher_and_state();
        let state: &P2pState = state.substate()?;

        state.p2p_connection_timeouts_dispatch(dispatcher, time)?;
        if let Some(opts) = state.recovery_candidate(time) {
            dispatcher.push(P2pConnectionOutgoingAction::Recover { opts: opts.clone() });
        }
        dispatcher.push(P2pConnectionOutgoingAction::RandomInit);
        dispatcher.push(P2pDisconnectionAction::RandomTry);

        state.p2p_connect_initial_peers(dispatcher, time);
        state.p2p_try_reconnect_disconnected_peers(dispatcher, time)?;
        state.p2p_discovery(dispatcher, time)?;

        #[cfg(feature = "p2p-libp2p")]
        {
            state.p2p_pnet_timeouts(dispatcher, time)?;
            state.p2p_select_timeouts(dispatcher, time)?;
            state.p2p_rpc_heartbeats(dispatcher, time)?;
            if let Some(kad) = state.network.scheduler.discovery_state.as_ref() {
                for (addr, peer_id, stream_id) in kad.expired_partial_frames(time) {
                    dispatcher.push(P2pNetworkKademliaStreamAction::Timeout {
                        addr,
                        peer_id,
                        stream_id,
                    });
                }
            }
            dispatcher.push(P2pNetworkPubsubAction::PruneMessages {});
        }

        state.rpc_timeouts(dispatcher, time)?;
        Ok(())
    }

    fn p2p_connection_timeouts_dispatch<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let timeouts = &self.config.timeouts;

        self.peers
            .iter()
            .filter_map(|(peer_id, peer)| {
                let state = peer.status.as_connecting()?;
                state
                    .is_timed_out(time, timeouts)
                    .then(|| (*peer_id, state.as_outgoing().is_some()))
            })
            .for_each(|(peer_id, is_outgoing)| match is_outgoing {
                true => dispatcher.push(P2pConnectionOutgoingAction::Timeout { peer_id }),
                false => dispatcher.push(P2pConnectionIncomingAction::Timeout { peer_id }),
            });

        Ok(())
    }

    fn p2p_try_reconnect_disconnected_peers<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        if self.already_has_min_peers() {
            return Ok(());
        }

        // Best first, and for libp2p only as many as we are short of the
        // target ([`P2pState::dial_candidates`]).
        self.dial_candidates(time)
            .into_iter()
            .cloned()
            .map(|opts| P2pConnectionOutgoingAction::Reconnect { opts, rpc_id: None })
            .for_each(|action| dispatcher.push(action));
        Ok(())
    }

    /// Dials the initial peers, our seeds, that aren't in the peer table:
    /// all of them at start, and later only while we want seed connections
    /// ([`P2pState::wants_seed_connections`]) and their backoff allows. A
    /// seed that left the peer table used to be dialled again at once, on
    /// the next tick, with no wait at all.
    fn p2p_connect_initial_peers<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        if self.already_has_min_peers() {
            return;
        }

        self.config
            .initial_peers
            .iter()
            .filter(|opts| !opts.is_libp2p() || self.wants_seed_connections())
            .filter(|opts| !self.peers.contains_key(opts.peer_id()) && self.may_dial(opts, time))
            .cloned()
            .for_each(|opts| {
                dispatcher.push(P2pConnectionOutgoingAction::Init {
                    opts,
                    rpc_id: None,
                    on_success: None,
                });
            });
    }

    fn rpc_timeouts<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        self.peer_rpc_timeouts(time)
            .into_iter()
            .for_each(|(peer_id, id, is_streaming)| {
                if is_streaming {
                    dispatcher.push(P2pChannelsStreamingRpcAction::Timeout { peer_id, id });
                } else {
                    dispatcher.push(P2pChannelsRpcAction::Timeout { peer_id, id });
                }
            });

        Ok(())
    }

    fn p2p_discovery<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let config = &self.config;
        let timeouts = &config.timeouts;

        if !config.peer_discovery {
            return Ok(());
        }

        for (&peer_id, _) in self
            .ready_peers_iter()
            .filter(|(_, peer)| peer.channels.signaling.discovery.is_ready())
        {
            dispatcher.push(P2pChannelsSignalingDiscoveryAction::RequestSend { peer_id });
            dispatcher.push(P2pChannelsSignalingDiscoveryAction::DiscoveryRequestSend { peer_id });
        }

        if let Some(_d) = config.timeouts.initial_peers {
            // ask initial peers
            // TODO: use RPC to ask initial peers
        }

        #[cfg(feature = "p2p-libp2p")]
        {
            if let Some(discovery_state) = self.network.scheduler.discovery_state() {
                // A lookup with a request that never ends is ended, and so
                // are its requests, so that the next one can start.
                if let Some(lookup) = discovery_state
                    .bootstrap_state()
                    .filter(|lookup| lookup.is_timed_out(time))
                {
                    dispatcher.push(P2pNetworkKademliaAction::BootstrapFinished);
                    for peer_id in lookup.requests.keys() {
                        if discovery_state.request(peer_id).is_some() {
                            dispatcher.push(crate::P2pNetworkKadRequestAction::Error {
                                peer_id: *peer_id,
                                error: "Kademlia lookup timed out".to_owned(),
                            });
                        }
                    }
                }

                // The first lookup is for our own id, at start. Later ones,
                // `kademlia_bootstrap` after the last ended, only while we
                // have fewer ready libp2p peers than the target, as OCaml's
                // helper looks for peers only below its low-water mark, and
                // for a random id, so that each finds other peers.
                //
                // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L145-L171>
                let first = !discovery_state.has_bootstrapped;
                if discovery_state.status.can_bootstrap(time, timeouts)
                    && (first || self.wants_more_libp2p_peers())
                {
                    let key = if first {
                        self.my_id()
                    } else {
                        random_lookup_key(time)
                    };
                    match P2pNetworkKadKey::try_from(&key) {
                        Ok(kad_key) => {
                            if discovery_state
                                .routing_table
                                .closest_peers(&kad_key)
                                .any(|_| true)
                            {
                                dispatcher.push(P2pNetworkKademliaAction::StartBootstrap { key });
                            }
                        }
                        Err(e) => bug_condition!("p2p discovery error: {:?}", e),
                    }
                }
            }
        }

        Ok(())
    }
}

/// A random peer id to look up, from the time, so that the timer stays
/// deterministic. go-libp2p-kad-dht refreshes its routing table the same way,
/// with lookups for random keys.
#[cfg(feature = "p2p-libp2p")]
fn random_lookup_key(time: Timestamp) -> crate::PeerId {
    use rand::Rng;
    let mut rng = mina_core::pseudo_rng(time);
    crate::identity::SecretKey::from_bytes(rng.gen())
        .public_key()
        .peer_id()
}

#[cfg(feature = "p2p-libp2p")]
impl P2pState {
    fn p2p_pnet_timeouts<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let timeouts = &self.config.timeouts;

        self.network
            .scheduler
            .connections
            .iter()
            .filter(|(_, state)| state.pnet.is_timed_out(time, timeouts))
            .map(|(addr, _)| P2pNetworkPnetAction::Timeout { addr: *addr })
            .for_each(|action| dispatcher.push(action));

        Ok(())
    }

    fn p2p_select_timeouts<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let timeouts = &self.config.timeouts;

        self.network
            .scheduler
            .connections
            .iter()
            .filter(|(_, state)| state.select_auth.is_timed_out(time, timeouts))
            .map(|(addr, _)| P2pNetworkSelectAction::Timeout {
                addr: *addr,
                kind: crate::SelectKind::Authentication,
            })
            .for_each(|action| dispatcher.push(action));

        self.network
            .scheduler
            .connections
            .iter()
            .filter(|(_, state)| state.select_mux.is_timed_out(time, timeouts))
            .map(|(addr, _)| P2pNetworkSelectAction::Timeout {
                addr: *addr,
                kind: crate::SelectKind::MultiplexingNoPeerId,
            })
            .for_each(|action| dispatcher.push(action));

        // Streams exist only once the noise handshake has named the peer, so
        // every connection with streams has a peer id. The scheduler uses it
        // to reset just the stream that timed out.
        self.network
            .scheduler
            .connections
            .iter()
            .filter_map(|(addr, state)| Some((addr, state.peer_id()?, state)))
            .flat_map(|(addr, peer_id, state)| {
                state
                    .streams
                    .iter()
                    .filter(|(_, stream)| stream.select.is_stream_timed_out(time, timeouts))
                    .map(|(stream_id, _)| (*addr, *peer_id, *stream_id))
            })
            .map(
                |(addr, peer_id, stream_id)| P2pNetworkSelectAction::Timeout {
                    addr,
                    kind: crate::SelectKind::Stream(peer_id, stream_id),
                },
            )
            .for_each(|action| dispatcher.push(action));

        Ok(())
    }

    fn p2p_rpc_heartbeats<State, Action>(
        &self,
        dispatcher: &mut Dispatcher<Action, State>,
        time: Timestamp,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let scheduler = &self.network.scheduler;

        // Heartbeats do not prove that a peer is reading its RPC responses.
        // Close only stalled streams, releasing their queued frames and work.
        const MAX_QUEUED_AGE: std::time::Duration = std::time::Duration::from_secs(30);
        const MAX_RESPONSE_AGE: std::time::Duration = std::time::Duration::from_secs(120);
        for (addr, connection) in &scheduler.connections {
            let Some(peer_id) = connection.peer_id().copied() else {
                continue;
            };
            let Some(P2pNetworkConnectionMuxState::Yamux(yamux)) = &connection.mux else {
                continue;
            };
            for (stream_id, stream) in &yamux.streams {
                if !scheduler
                    .rpc_incoming_streams
                    .get(&peer_id)
                    .is_some_and(|streams| streams.contains_key(stream_id))
                {
                    continue;
                }
                let stalled = stream.pending_since.is_some_and(|since| {
                    time.checked_sub(since)
                        .is_some_and(|elapsed| elapsed > MAX_QUEUED_AGE)
                });
                let queued_too_long = stream.pending_first_at.is_some_and(|since| {
                    time.checked_sub(since)
                        .is_some_and(|elapsed| elapsed > MAX_RESPONSE_AGE)
                });
                let work_too_long = scheduler
                    .rpc_incoming_streams
                    .get(&peer_id)
                    .and_then(|streams| streams.get(stream_id))
                    .is_some_and(|rpc| {
                        rpc.pending_incoming.values().any(|query| {
                            time.checked_sub(query.started_at)
                                .is_some_and(|elapsed| elapsed > MAX_RESPONSE_AGE)
                        })
                    });
                if stalled || queued_too_long || work_too_long {
                    dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                        addr: *addr,
                        stream_id: *stream_id,
                        data: Data::default(),
                        flags: YamuxFlags::RST,
                    });
                    dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                        addr: *addr,
                        peer_id,
                        stream_id: *stream_id,
                        protocol: None,
                        reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                    });
                }
            }
        }

        for (peer_id, streams) in &scheduler.rpc_incoming_streams.0 {
            for (stream_id, rpc) in streams {
                if rpc.is_timed_out(time) {
                    dispatcher.push(P2pNetworkRpcAction::Timeout {
                        addr: rpc.addr,
                        peer_id: *peer_id,
                        stream_id: *stream_id,
                    });
                }
            }
        }

        scheduler
            .rpc_incoming_streams
            .0
            .iter()
            .chain(&scheduler.rpc_outgoing_streams.0)
            .flat_map(|(peer_id, state)| {
                state
                    .iter()
                    .filter(|(_, s)| s.should_send_heartbeat(time))
                    .map(|(stream_id, state)| P2pNetworkRpcAction::HeartbeatSend {
                        addr: state.addr,
                        peer_id: *peer_id,
                        stream_id: *stream_id,
                    })
            })
            .for_each(|action| dispatcher.push(action));

        Ok(())
    }
}
