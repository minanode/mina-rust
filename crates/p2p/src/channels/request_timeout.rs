//! When our request to a peer has timed out.
//!
//! A web node far from its relay, 150 to 250 ms away, gets its bootstrap at a
//! few hundred kilobytes a second. An answer can then wait behind megabytes
//! of staged-ledger parts or a large block on the same connection, and a
//! plain timeout counted from the request dropped the relay, the web node's
//! only peer, in the middle of a transfer that was still going
//! (WEB-FAR-RELAY-LOOP). For a WebRTC peer a request therefore times out on
//! silence: when the peer has sent nothing at all, on any channel, for the
//! request's timeout. A peer that keeps sending other things but never
//! answers is still caught, by a ceiling on the whole wait.
//!
//! A request that timed out once is retried elsewhere, if there is anywhere
//! else, and a second timeout drops the peer. For a WebRTC peer the second
//! timeout is the ceiling, an absolute deadline counted from when the request
//! was sent, which nothing extends: neither the first timeout nor bytes from
//! the peer. Seconds of silence on a live link are loss recovery (SCTP backs
//! its retransmission timer off) or a busy peer, and dropping the peer
//! restarts whatever it was sending, so a short silence after the first
//! timeout does not drop it sooner. A dead link is noticed by the transport
//! itself (ICE consent, SCTP retransmission limits), which closes the
//! connection. libp2p peers keep the plain timeout, twice.

use std::time::Duration;

use redux::Timestamp;

/// A WebRTC request times out after at most this many times its timeout,
/// however much else the peer sends meanwhile.
pub const MAX_TIMEOUT_STRETCH: u32 = 10;

/// The ceiling is never shorter than this.
pub const MIN_STRETCHED_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest a WebRTC request with `timeout` can wait.
pub fn stretched_timeout(timeout: Duration) -> Duration {
    timeout
        .saturating_mul(MAX_TIMEOUT_STRETCH)
        .max(MIN_STRETCHED_TIMEOUT)
}

/// Whether a request waiting since `since` has timed out at `now`.
/// `last_received` is when a WebRTC peer last sent us anything
/// ([`super::P2pChannelsState::last_received`]); `None`, for a libp2p peer,
/// keeps the plain timeout.
pub fn request_timed_out(
    since: Timestamp,
    timeout: Duration,
    now: Timestamp,
    last_received: Option<Timestamp>,
) -> bool {
    let Some(waited) = now.checked_sub(since) else {
        return false;
    };
    let Some(last_received) = last_received else {
        return waited >= timeout;
    };
    let quiet = now
        .checked_sub(last_received.max(since))
        .unwrap_or_default();
    quiet >= timeout || waited >= stretched_timeout(timeout)
}

/// Whether a request sent at `sent` that timed out once, at `timed_out_at`,
/// has timed out again at `now`: the peer is then dropped. For a libp2p peer
/// a full timeout after the first; for a WebRTC peer at the ceiling counted
/// from `sent`, [`stretched_timeout`], however the first timeout came.
pub fn timed_out_request_expired(
    sent: Timestamp,
    timed_out_at: Timestamp,
    timeout: Duration,
    now: Timestamp,
    last_received: Option<Timestamp>,
) -> bool {
    match last_received {
        None => request_timed_out(timed_out_at, timeout, now, None),
        Some(_) => now
            .checked_sub(sent)
            .is_some_and(|waited| waited >= stretched_timeout(timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    const FOUR: Duration = Duration::from_secs(4);

    #[test]
    fn a_libp2p_request_keeps_the_plain_timeout() {
        assert!(!request_timed_out(at(10), FOUR, at(13), None));
        assert!(request_timed_out(at(10), FOUR, at(14), None));
        assert!(!request_timed_out(at(10), FOUR, at(9), None));
    }

    /// Bytes from the peer, after the request, push the timeout back.
    #[test]
    fn a_webrtc_request_times_out_on_silence() {
        // Nothing since the request: the plain timeout.
        assert!(!request_timed_out(at(10), FOUR, at(13), Some(at(0))));
        assert!(request_timed_out(at(10), FOUR, at(14), Some(at(0))));
        // The peer sent something at 12 s and 20 s: counted from there.
        assert!(!request_timed_out(at(10), FOUR, at(15), Some(at(12))));
        assert!(request_timed_out(at(10), FOUR, at(16), Some(at(12))));
        assert!(!request_timed_out(at(10), FOUR, at(23), Some(at(20))));
        assert!(request_timed_out(at(10), FOUR, at(24), Some(at(20))));
    }

    /// A peer that keeps sending but never answers is caught by the ceiling.
    #[test]
    fn a_webrtc_request_has_a_ceiling() {
        assert_eq!(stretched_timeout(FOUR), Duration::from_secs(60));
        assert_eq!(
            stretched_timeout(Duration::from_secs(30)),
            Duration::from_secs(300)
        );
        assert!(!request_timed_out(at(10), FOUR, at(69), Some(at(69))));
        assert!(request_timed_out(at(10), FOUR, at(70), Some(at(70))));
    }

    /// For a request sent at 0 that first timed out at 10 s, a libp2p peer is
    /// dropped a timeout after the first; a WebRTC peer at the ceiling counted
    /// from the send, 60 s, whether it went silent or kept sending.
    #[test]
    fn a_second_timeout_drops_a_webrtc_peer_at_the_ceiling_from_the_send() {
        assert!(!timed_out_request_expired(
            at(0),
            at(10),
            FOUR,
            at(13),
            None
        ));
        assert!(timed_out_request_expired(at(0), at(10), FOUR, at(14), None));
        for last_received in [at(0), at(30), at(59)] {
            let last_received = Some(last_received);
            assert!(!timed_out_request_expired(
                at(0),
                at(10),
                FOUR,
                at(59),
                last_received
            ));
            assert!(timed_out_request_expired(
                at(0),
                at(10),
                FOUR,
                at(60),
                last_received
            ));
        }
        // A first timeout at the ceiling itself leaves nothing more to wait.
        assert!(request_timed_out(at(0), FOUR, at(60), Some(at(60))));
        assert!(timed_out_request_expired(
            at(0),
            at(60),
            FOUR,
            at(60),
            Some(at(60))
        ));
    }
}
