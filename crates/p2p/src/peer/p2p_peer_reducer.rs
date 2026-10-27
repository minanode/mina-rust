use mina_core::{bug_condition, Substate};
use redux::{ActionWithMeta, Timestamp};

use crate::{P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, P2pState};

use super::P2pPeerAction;

impl P2pPeerState {
    /// Substate is accessed
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, P2pState>,
        action: ActionWithMeta<P2pPeerAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let p2p_state = state_context.get_substate_mut()?;
        let (action, meta) = action.split();

        match action {
            P2pPeerAction::Discovered { peer_id, dial_opts } => {
                if !p2p_state.peers.contains_key(&peer_id)
                    && p2p_state.peers.len() >= p2p_state.config.limits.max_peers_in_state()
                {
                    // The enabling condition found a peer to forget.
                    if let Some(evicted) = p2p_state.peer_to_evict(meta.time()) {
                        let failures = p2p_state
                            .dial_backoff
                            .peer(&evicted)
                            .map_or(0, |e| e.failures);
                        p2p_state.peers.remove(&evicted);
                        p2p_state.dial_addresses.remove(&evicted);
                        mina_core::debug!(meta.time();
                            kind = "P2pPeerEvicted",
                            summary = "forgot a failing peer to make room for a discovered one",
                            peer_id = display(evicted),
                            failures = failures,
                            discovered = display(peer_id),
                        );
                    }
                }
                p2p_state
                    .peers
                    .entry(peer_id)
                    .or_insert_with(|| P2pPeerState {
                        is_libp2p: true,
                        dial_opts: None,
                        identify: None,
                        status: P2pPeerStatus::Disconnected {
                            time: Timestamp::ZERO,
                        },
                    });

                if let Some(dial_opts) = dial_opts {
                    p2p_state.add_dial_address(peer_id, dial_opts, None);
                }
                Ok(())
            }
            P2pPeerAction::Ready { peer_id, incoming } => {
                let Some(peer) = p2p_state.peers.get_mut(&peer_id) else {
                    return Ok(());
                };
                peer.status = P2pPeerStatus::Ready(Box::new(P2pPeerStatusReady::new(
                    incoming,
                    meta.time(),
                    &p2p_state.config.enabled_channels,
                )));
                let is_libp2p = peer.is_libp2p;
                if is_libp2p {
                    p2p_state
                        .dial_backoff
                        .on_ready(peer_id, incoming, meta.time());
                    if !incoming {
                        p2p_state.record_dial_address_success(peer_id, meta.time());
                    }
                }

                if !is_libp2p {
                    let (dispatcher, state) = state_context.into_dispatcher_and_state();
                    let state: &P2pState = state.substate()?;
                    state.channels_init(dispatcher, peer_id);
                }

                Ok(())
            }
            P2pPeerAction::BestTipUpdate { peer_id, best_tip } => {
                let Some(peer) = p2p_state.peers.get_mut(&peer_id) else {
                    bug_condition!("Peer state not found for `P2pPeerAction::BestTipUpdate`");
                    return Ok(());
                };
                let is_libp2p = peer.is_libp2p;
                let Some(peer) = peer.status.as_ready_mut() else {
                    bug_condition!("Peer not ready for `P2pPeerAction::BestTipUpdate`");
                    return Ok(());
                };
                peer.best_tip = Some(best_tip.clone());
                // Whether we may gossip it to libp2p peers depends on this.
                p2p_state
                    .network
                    .scheduler
                    .broadcast_state
                    .block_sources
                    .record(best_tip.hash(), is_libp2p);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = &p2p_state.callbacks.on_p2p_peer_best_tip_update {
                    dispatcher.push_callback(callback.clone(), best_tip);
                }
                Ok(())
            }
            P2pPeerAction::Remove { peer_id } => {
                if p2p_state.peers.remove(&peer_id).is_none() {
                    bug_condition!(
                        "Missing state for peer {peer_id} action: `P2pPeerAction::Remove`"
                    );
                }
                p2p_state.dial_addresses.remove(&peer_id);

                Ok(())
            }
            P2pPeerAction::Stale {
                peer_id,
                addr,
                answered_by,
            } => {
                // go-libp2p fails such a dial (noise's "peer id mismatch") and
                // backs off the address, but keeps the record, and OCaml
                // ignores the error; we redialled it every 30 s forever. We
                // give up on the peer id at that address instead: OCaml
                // makes a new libp2p key at each start, so the old id won't
                // come back there.
                //
                // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/gossip_net/libp2p.ml#L263-L274>
                // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L86-L93>
                // The peer keeps its dial options until the dial's error is
                // handled (`P2pConnectionOutgoingAction::Error`): the closing
                // connection is matched to the peer by them.
                p2p_state
                    .dial_backoff
                    .on_stale(peer_id, addr, answered_by, meta.time());
                #[cfg(feature = "p2p-libp2p")]
                let removed_from_routing_table = p2p_state
                    .network
                    .scheduler
                    .discovery_state
                    .as_mut()
                    .is_some_and(|discovery| discovery.routing_table.remove(&peer_id));
                #[cfg(not(feature = "p2p-libp2p"))]
                let removed_from_routing_table = false;
                mina_core::debug!(meta.time();
                    kind = "P2pStalePeerId",
                    summary = "address answered as another peer id; not dialling this one there",
                    peer_id = display(peer_id),
                    addr = display(addr),
                    answered_by = display(answered_by),
                    removed_from_routing_table = removed_from_routing_table,
                );
                Ok(())
            }
        }
    }
}
