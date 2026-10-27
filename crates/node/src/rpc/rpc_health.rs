//! Liveness and readiness contracts behind `GET /healthz` and `GET /readyz`.
//!
//! Liveness only says that the node answers: the HTTP server is up and the
//! state machine handles requests. It does not depend on peers or sync, so a
//! liveness probe keeps passing while the node bootstraps or catches up and an
//! orchestrator does not restart a healthy node. Readiness says the node is
//! worth sending traffic to: it is synced, recently, and has ready peers.
//!
//! This follows OCaml 4.0.2's `mina-healthcheck`: `daemon-status` (the
//! liveness probe) exits 0 whenever the daemon answers, and `ready` requires a
//! synced node with enough peers.

use std::time::Duration;

use redux::Timestamp;

use super::{RpcHealthCheckResponse, RpcReadinessCheckResponse};
use crate::transition_frontier::sync::{SyncPhase, TransitionFrontierSyncState};

/// How long ago the node may have last reached `Synced` and still be ready.
pub const READINESS_SYNC_THRESHOLD: Duration = Duration::from_secs(60 * 3 * 10);

/// How many ready peers a node needs to be ready.
///
/// OCaml's `mina-healthcheck ready --min-peers` defaults to 2; a Rust node
/// that was healthy before this split needed one ready peer, so readiness
/// keeps that bar.
pub const READINESS_MIN_PEERS: usize = 1;

/// Liveness: answering at all is the whole check.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/mina_healthcheck/mina_healthcheck.ml#L94-L122>
pub fn health_check() -> RpcHealthCheckResponse {
    Ok(())
}

/// Readiness: synced within [`READINESS_SYNC_THRESHOLD`] and at least
/// [`READINESS_MIN_PEERS`] ready peers. Every failing condition is reported,
/// joined by `, `, as OCaml's `readiness_problems` does.
///
/// Synced is the phase the node reports (`phase`,
/// [`crate::transition_frontier::sync::reported_sync_phase`], F-dash3), not
/// the sync's own state: that is `Synced` on the genesis block at the start,
/// and on a restored tip, before any peer has sent a best tip. OCaml's extra
/// "chain length = highest received" condition is covered in part: a peer's
/// better block, once its proof is verified, makes the node catch up, and
/// not ready; an unverified claim does not.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_graphql_client/client.ml#L1036-L1064>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/mina_healthcheck/lib/mina_healthcheck_lib.ml#L80-L93>
pub fn readiness_check(
    phase: SyncPhase,
    sync: &TransitionFrontierSyncState,
    ready_peers: usize,
    now: Timestamp,
) -> RpcReadinessCheckResponse {
    let mut problems = Vec::new();
    match sync {
        _ if phase != SyncPhase::Synced => problems.push("not synced".to_owned()),
        TransitionFrontierSyncState::Synced { time }
            if now.checked_sub(*time) <= Some(READINESS_SYNC_THRESHOLD) => {}
        TransitionFrontierSyncState::Synced { time } => problems.push(format!(
            "Synced {:?} ago, which is more than the threshold {:?}",
            now.checked_sub(*time),
            READINESS_SYNC_THRESHOLD
        )),
        _ => problems.push("not synced".to_owned()),
    }
    if ready_peers < READINESS_MIN_PEERS {
        problems.push(format!("peer count {ready_peers} < {READINESS_MIN_PEERS}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(secs * SEC)
    }

    fn synced_at(secs: u64) -> TransitionFrontierSyncState {
        TransitionFrontierSyncState::Synced { time: at(secs) }
    }

    const SYNCED: SyncPhase = SyncPhase::Synced;

    #[test]
    fn liveness_does_not_depend_on_peers_or_sync() {
        // The brief and OCaml's `daemon-status`: a bootstrapping node with no
        // peers is alive.
        assert_eq!(health_check(), Ok(()));
    }

    #[test]
    fn ready_when_recently_synced_with_a_peer() {
        assert_eq!(
            readiness_check(SYNCED, &synced_at(1000), 1, at(1000)),
            Ok(())
        );
        // The 30-minute threshold is inclusive.
        assert_eq!(
            readiness_check(SYNCED, &synced_at(1000), 3, at(1000 + 1800)),
            Ok(())
        );
    }

    #[test]
    fn not_ready_while_bootstrapping() {
        assert_eq!(
            readiness_check(
                SyncPhase::Bootstrap,
                &TransitionFrontierSyncState::Idle,
                5,
                at(1000)
            ),
            Err("not synced".to_owned())
        );
    }

    #[test]
    fn not_ready_when_sync_is_stale() {
        assert_eq!(
            readiness_check(SYNCED, &synced_at(1000), 5, at(1000 + 1801)),
            Err("Synced Some(1801s) ago, which is more than the threshold 1800s".to_owned())
        );
    }

    #[test]
    fn not_ready_without_peers() {
        // OCaml's `ready` fails with too few peers even when synced.
        assert_eq!(
            readiness_check(SYNCED, &synced_at(1000), 0, at(1000)),
            Err("peer count 0 < 1".to_owned())
        );
        // Without peers the node reports `Connecting`, so not synced either.
        assert_eq!(
            readiness_check(SyncPhase::Connecting, &synced_at(1000), 0, at(1000)),
            Err("not synced, peer count 0 < 1".to_owned())
        );
    }

    /// F-dash3: the sync is `Synced` on the genesis block at the start, and
    /// on a restored tip, before any peer has sent a best tip. The node isn't
    /// ready until it reports `Synced`.
    #[test]
    fn not_ready_while_synced_before_the_network_is_heard() {
        for phase in [
            SyncPhase::Listening,
            SyncPhase::Bootstrap,
            SyncPhase::Catchup,
        ] {
            assert_eq!(
                readiness_check(phase, &synced_at(1000), 3, at(1000)),
                Err("not synced".to_owned()),
                "{phase}"
            );
        }
    }

    #[test]
    fn every_problem_is_reported() {
        assert_eq!(
            readiness_check(
                SyncPhase::Bootstrap,
                &TransitionFrontierSyncState::Idle,
                0,
                at(1000)
            ),
            Err("not synced, peer count 0 < 1".to_owned())
        );
    }
}
