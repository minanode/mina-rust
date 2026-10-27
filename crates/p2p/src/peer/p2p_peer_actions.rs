use std::net::SocketAddr;

use mina_core::{block::ArcBlockWithHash, ActionEvent};
use serde::{Deserialize, Serialize};

use crate::{connection::outgoing::P2pConnectionOutgoingInitOpts, P2pState, PeerId};

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = debug, fields(display(peer_id), debug(dial_opts), best_tip = display(&best_tip.hash), incoming))]
pub enum P2pPeerAction {
    /// Peer is discovered.
    #[action_event(level = debug)]
    Discovered {
        peer_id: PeerId,
        dial_opts: Option<P2pConnectionOutgoingInitOpts>,
    },
    /// Peer is ready.
    Ready { peer_id: PeerId, incoming: bool },
    /// Peer's best tip is updated.
    BestTipUpdate {
        peer_id: PeerId,
        best_tip: ArcBlockWithHash,
    },
    /// Remove peer from state
    Remove { peer_id: PeerId },
    /// Our dial to `peer_id` at `addr` was answered by another peer id,
    /// `answered_by`: the record we dialled is stale.
    #[action_event(level = debug, fields(display(peer_id), display(addr), display(answered_by)))]
    Stale {
        peer_id: PeerId,
        addr: SocketAddr,
        answered_by: PeerId,
    },
}

impl P2pPeerAction {
    pub fn peer_id(&self) -> &PeerId {
        match self {
            Self::Discovered { peer_id, .. } => peer_id,
            Self::Ready { peer_id, .. } => peer_id,
            Self::BestTipUpdate { peer_id, .. } => peer_id,
            Self::Remove { peer_id } => peer_id,
            Self::Stale { peer_id, .. } => peer_id,
        }
    }
}

impl redux::EnablingCondition<P2pState> for P2pPeerAction {
    fn is_enabled(&self, state: &P2pState, time: redux::Timestamp) -> bool {
        match self {
            P2pPeerAction::Discovered { peer_id, dial_opts } => {
                peer_id != &state.my_id()
                    && state
                        .peers
                        .get(peer_id)
                        .is_none_or(|_| {
                            dial_opts.as_ref().is_some_and(|candidate| {
                                !state.dial_address_known(peer_id, candidate)
                            })
                        })
                    // A full table makes room for a peer we never tried by
                    // forgetting a failing one (I18b): discovery used to stop
                    // there, with the table full of failed dials.
                    && (state.peers.contains_key(peer_id)
                        || state.peers.len() < state.config.limits.max_peers_in_state()
                        || (state.is_new_libp2p_peer(peer_id, dial_opts.as_ref())
                            && state.peer_to_evict(time).is_some()))
                    // Not at an address that answered as another peer id.
                    && !state.dial_backoff.is_stale(
                        peer_id,
                        dial_opts.as_ref().and_then(|o| o.libp2p_socket_addr()),
                        time,
                        &state.dial_backoff_config(),
                    )
            }
            P2pPeerAction::Ready { peer_id, .. } => state
                .peers
                .get(peer_id)
                .is_some_and(|p| p.status.is_connecting_success()),
            P2pPeerAction::BestTipUpdate { peer_id, .. } => {
                // TODO: don't enable if block inferior than existing peer's
                // best tip.
                state.get_ready_peer(peer_id).is_some()
            }
            P2pPeerAction::Remove { peer_id } => {
                state.peers.len() > state.config.limits.min_peers_in_state()
                    && state.peers.contains_key(peer_id)
            }
            P2pPeerAction::Stale {
                peer_id,
                answered_by,
                ..
            } => peer_id != answered_by,
        }
    }
}
