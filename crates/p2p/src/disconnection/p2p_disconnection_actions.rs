use std::time::Duration;

use mina_core::ActionEvent;
use serde::{Deserialize, Serialize};

use super::P2pDisconnectionReason;
use crate::{P2pPeerStatus, P2pState, PeerId};

pub type P2pDisconnectionActionWithMetaRef<'a> = redux::ActionWithMeta<&'a P2pDisconnectionAction>;

const RANDOM_DISCONNECTION_TRY_FREQUENCY: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = debug)]
pub enum P2pDisconnectionAction {
    RandomTry,
    /// Initialize disconnection.
    #[action_event(expr(log_disconnection(context, peer_id, reason)))]
    Init {
        peer_id: PeerId,
        reason: P2pDisconnectionReason,
    },
    /// Peer disconnection.
    #[action_event(fields(display(peer_id)), level = debug)]
    PeerClosed {
        peer_id: PeerId,
    },
    #[action_event(fields(display(peer_id)), level = debug)]
    FailedCleanup {
        peer_id: PeerId,
    },
    /// Finish disconnecting from a peer.
    #[action_event(fields(display(peer_id)), level = debug)]
    Finish {
        peer_id: PeerId,
    },
}

fn log_disconnection<T: mina_core::log::EventContext>(
    context: &T,
    peer_id: &PeerId,
    reason: &P2pDisconnectionReason,
) {
    if reason.is_misbehaviour() {
        mina_core::action_warn!(context, kind = "P2pDisconnectionInit", peer_id = %peer_id, reason = %reason);
    } else {
        mina_core::action_debug!(context, kind = "P2pDisconnectionInit", peer_id = %peer_id, reason = %reason);
    }
}

impl redux::EnablingCondition<P2pState> for P2pDisconnectionAction {
    fn is_enabled(&self, state: &P2pState, time: redux::Timestamp) -> bool {
        match self {
            P2pDisconnectionAction::RandomTry => time
                .checked_sub(state.last_random_disconnection_try)
                .is_some_and(|dur| dur >= RANDOM_DISCONNECTION_TRY_FREQUENCY),
            P2pDisconnectionAction::Init { peer_id, .. } => {
                state.peers.get(peer_id).is_some_and(|peer| {
                    !peer.status.is_disconnected_or_disconnecting() && !peer.status.is_error()
                })
            }
            P2pDisconnectionAction::Finish { peer_id } => {
                state.peers.get(peer_id).is_some_and(|peer| {
                    !matches!(peer.status, P2pPeerStatus::Disconnected { .. })
                        && !peer.status.is_error()
                })
            }
            P2pDisconnectionAction::PeerClosed { peer_id, .. } => {
                state.peers.get(peer_id).is_some_and(|peer| {
                    !peer.status.is_disconnected_or_disconnecting() && !peer.status.is_error()
                })
            }
            P2pDisconnectionAction::FailedCleanup { peer_id } => state
                .peers
                .get(peer_id)
                .is_some_and(|peer| !peer.is_libp2p() && peer.status.is_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::{field::Visit, Event, Level, Subscriber};
    use tracing_subscriber::{
        layer::{Context, SubscriberExt},
        Layer,
    };

    struct TestContext;
    static ZERO: u64 = 0;

    impl mina_core::log::EventContext for TestContext {
        fn timestamp(&self) -> redux::Timestamp {
            redux::Timestamp::new(0)
        }
        fn time(&self) -> &'_ dyn tracing::Value {
            &ZERO
        }
        fn node_id(&self) -> &'_ dyn tracing::Value {
            &ZERO
        }
        fn log_node_id(&self) -> bool {
            false
        }
    }

    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }
    }

    struct Capture(Arc<Mutex<Vec<(Level, String)>>>);
    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.0));
        }
    }

    #[test]
    fn initiated_disconnect_logs_peer_and_reason_at_expected_level() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::Registry::default().with(Capture(events.clone()));
        let peer_id: PeerId = "2bEgBrPTzL8wov2D4Kz34WVLCxR4uCarsBmHYXWKQA5wvBQzd9H"
            .parse()
            .unwrap();
        tracing::subscriber::with_default(subscriber, || {
            log_disconnection(
                &TestContext,
                &peer_id,
                &P2pDisconnectionReason::SnarkPoolVerifyError,
            );
            log_disconnection(&TestContext, &peer_id, &P2pDisconnectionReason::FreeUpSpace);
        });
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, Level::WARN);
        assert_eq!(events[1].0, Level::DEBUG);
        for (_, fields) in events.iter() {
            assert!(fields.contains("peer_id="));
            assert!(fields.contains("reason="));
            assert!(fields.contains(&peer_id.to_string()));
        }
        assert!(events[0].1.contains("failed to verify snark pool diff"));
    }
}
