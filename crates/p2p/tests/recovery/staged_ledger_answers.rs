//! Staged-ledger answers within the send budgets, through the server's real
//! RPC and Yamux reducers. The peer is a model of a go-yamux reader: it takes
//! the frames that reach Noise and grants receive window as it reads them.
use super::*;
use crate::staged_ledgers::{expected_payload, heavy_staged_ledger, none_payload, staged_ledger};
use mina_p2p::{
    channels::{rpc::StagedLedgerAuxAndPendingCoinbases, P2pChannelsAction},
    Data, Limit, P2pLimits, P2pNetworkNoiseAction, STAGED_LEDGER_FIRST_RESERVATION,
};
use mina_p2p_messages::rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2;
use std::sync::Arc;

const MIB: usize = 1024 * 1024;
/// The receive window a stream starts with, in go-yamux and ours.
const WINDOW: usize = 256 * 1024;
/// RPC framing around the payload: the eight-byte length and the message
/// header of a response with a one-byte id.
const FRAMING: usize = 8 + 2;
/// The released server's ceiling for a fresh stream: 64 MiB of queue and the
/// initial window.
const OLD_CEILING: usize = 64 * MIB + WINDOW;
/// Devnet's answer on 2026-11-18, root 3NLww7cA..., with its RPC framing.
const DEVNET: usize = 173_716_577 + FRAMING;
/// The default limit on a served answer.
const CAP: usize = 192 * MIB;

fn is_reset(action: &P2pAction) -> bool {
    matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset { .. }
        ))
    )
}

fn yamux_stream(s: &mut State, addr: ConnectionAddr, stream_id: u32) -> &mut YamuxStreamState {
    let P2pNetworkConnectionMuxState::Yamux(yamux) =
        s.0.network
            .scheduler
            .connections
            .get_mut(&addr)
            .unwrap()
            .mux
            .as_mut()
            .unwrap();
    yamux.streams.get_mut(&stream_id).unwrap()
}

/// Peer `n` asks for a staged ledger on a new stream `stream_id`. Returns the
/// query's id, or what the server sent if it refused the query.
fn ask(s: &mut State, addr: ConnectionAddr, n: u8, stream_id: u32) -> Result<u64, Vec<P2pAction>> {
    assert!(open(s, addr, stream_id));
    let mut bytes = Vec::new();
    NeedsLength(StateHash::zero().inner().0.clone())
        .binprot_write(&mut bytes)
        .unwrap();
    let mut rpc = P2pNetworkRpcState::new(addr, stream_id);
    rpc.is_incoming = true;
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(n))
        .or_default()
        .insert(stream_id, rpc);
    let actions = apply_traced(
        s,
        P2pNetworkRpcAction::IncomingMessage {
            addr,
            peer_id: peer(n),
            stream_id,
            message: RpcMessage::Query {
                header: QueryHeader {
                    tag: GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME.into(),
                    version: GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
                    id: 1,
                },
                bytes: bytes.into(),
            },
        },
        0,
    );
    let Some(request) = actions
        .iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Channels(P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::RequestReceived { .. }
                ))
            )
        })
        .cloned()
    else {
        return Err(actions);
    };
    let P2pAction::Channels(P2pChannelsAction::Rpc(P2pChannelsRpcAction::RequestReceived {
        id,
        ..
    })) = &request
    else {
        unreachable!()
    };
    let id = *id;
    assert!(request.is_enabled(&s.0, at(0)));
    apply(s, request, 0);
    Ok(id)
}

/// Applies `action`, then every enabled action it dispatches, depth first as
/// the store does, and returns them all. The Yamux frames handed to Noise are
/// returned but not applied: they are what goes on the wire.
fn drive(s: &mut State, action: impl Into<P2pAction>, t: u64) -> Vec<P2pAction> {
    fn go(s: &mut State, action: P2pAction, t: u64, all: &mut Vec<P2pAction>) {
        for child in apply_traced(s, action, t) {
            let to_noise = matches!(
                child,
                P2pAction::Network(P2pNetworkAction::Noise(
                    P2pNetworkNoiseAction::OutgoingData { .. }
                ))
            );
            if !to_noise && child.is_enabled(&s.0, at(t)) {
                go(s, child.clone(), t, all);
            }
            all.push(child);
        }
    }
    let mut all = Vec::new();
    go(s, action.into(), t, &mut all);
    all
}

/// The server answers peer `n`'s query `id` with `parts`.
fn answer(
    s: &mut State,
    n: u8,
    id: u64,
    parts: Arc<StagedLedgerAuxAndPendingCoinbases>,
    t: u64,
) -> Vec<P2pAction> {
    drive(
        s,
        P2pChannelsRpcAction::ResponseSend {
            peer_id: peer(n),
            id,
            response: Some(Box::new(
                P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(parts),
            )),
        },
        t,
    )
}

/// A reader of one stream with go-yamux's window: it starts at 256 KiB, the
/// reader grants what it has read once that is half the window, and the
/// window doubles at each grant up to 16 MiB, as go-yamux's auto-tuning does
/// for a reader that keeps up.
struct Reader {
    stream_id: u32,
    window: usize,
    max: usize,
    unacknowledged: usize,
    received: Vec<u8>,
}

impl Reader {
    fn new(stream_id: u32) -> Self {
        Self {
            stream_id,
            window: WINDOW,
            max: WINDOW,
            unacknowledged: 0,
            received: Vec::new(),
        }
    }

    /// Reads the data frames for its stream among `actions`, checking that
    /// each fits the window. Returns the window update to send, if any.
    fn read(&mut self, actions: &[P2pAction]) -> Option<u32> {
        for action in actions {
            let P2pAction::Network(P2pNetworkAction::Noise(P2pNetworkNoiseAction::OutgoingData {
                data,
                ..
            })) = action
            else {
                continue;
            };
            let header = &data[..12];
            let stream_id = u32::from_be_bytes(header[4..8].try_into().unwrap());
            if header[1] != 0 || stream_id != self.stream_id {
                continue;
            }
            let flags = u16::from_be_bytes(header[2..4].try_into().unwrap());
            assert_eq!(flags & !YamuxFlags::ACK.bits(), 0, "no flags but ACK, once");
            let len = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
            assert_eq!(data.len(), 12 + len);
            assert!(
                len <= self.window,
                "{len} bytes in a window of {}",
                self.window
            );
            self.window -= len;
            self.unacknowledged += len;
            self.received.extend_from_slice(&data[12..]);
        }
        if self.unacknowledged < self.max / 2 {
            return None;
        }
        self.max = (self.max * 2).min(16 * MIB);
        let grant = self.max - self.window;
        self.window = self.max;
        self.unacknowledged = 0;
        Some(grant as u32)
    }
}

/// Peer `addr` grants `delta` bytes of window on `stream_id`.
fn grant(
    s: &mut State,
    addr: ConnectionAddr,
    stream_id: u32,
    delta: u32,
    t: u64,
) -> Vec<P2pAction> {
    let mut bytes = vec![0, 1, 0, 0];
    bytes.extend_from_slice(&stream_id.to_be_bytes());
    bytes.extend_from_slice(&delta.to_be_bytes());
    apply(
        s,
        P2pNetworkYamuxAction::IncomingData {
            addr,
            data: bytes.into(),
        },
        t,
    );
    drive(s, P2pNetworkYamuxAction::IncomingFrame { addr }, t)
}

/// The RPC message an answer is: its eight-byte length, the message header of
/// a response to the wire id 1, and the payload.
fn message(payload: &[u8]) -> Vec<u8> {
    RpcMessage::Response {
        header: mina_p2p_messages::rpc_kernel::ResponseHeader { id: 1 },
        bytes: Data::from(payload.to_vec()),
    }
    .into_bytes()
}

fn queued(s: &State) -> usize {
    s.0.network.scheduler.queued_outgoing_bytes(&peer(1)).0
}

fn reserved(s: &State, n: u8) -> (usize, usize) {
    s.0.network.scheduler.reserved_rpc_response_bytes(&peer(n))
}

/// Whether the budgets take `len` bytes on peer `n`'s stream, once the
/// query's own reservation is released, as the server decides before it
/// encodes an answer.
fn admitted(s: &State, addr: ConnectionAddr, n: u8, stream_id: u32, len: usize) -> bool {
    let released = s.0.network.scheduler.rpc_incoming_streams[&peer(n)][&stream_id]
        .pending_incoming
        .values()
        .map(|query| query.reserved_bytes)
        .sum();
    s.0.network.scheduler.outgoing_data_admitted(
        &addr,
        &peer(n),
        stream_id,
        len,
        released,
        &s.0.config.limits,
    )
}

/// Answers over the released server's ceiling, today's devnet answer and
/// answers up to the limit fit the default budgets, even with no window left;
/// the released budgets took none of them past the ceiling.
#[test]
fn default_budgets_take_answers_up_to_the_limit() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    ask(&mut s, addr, 1, 2).unwrap();
    for len in [OLD_CEILING, OLD_CEILING + 1, DEVNET, CAP] {
        assert!(admitted(&s, addr, 1, 2, len), "{len} bytes");
    }
    yamux_stream(&mut s, addr, 2).window_theirs = 0;
    assert!(admitted(&s, addr, 1, 2, CAP));
    assert!(!admitted(&s, addr, 1, 2, CAP + 1));

    let mut old = state(&[]);
    old.0.config.limits = old
        .0
        .config
        .limits
        .with_yamux_pending_outgoing_peer_share(Some(64 * MIB))
        .with_rpc_inflight_peer_share(Some(128 * MIB));
    let addr = connection(&mut old);
    ask(&mut old, addr, 1, 2).unwrap();
    assert!(admitted(&old, addr, 1, 2, OLD_CEILING));
    assert!(!admitted(&old, addr, 1, 2, OLD_CEILING + 1));
    assert!(!admitted(&old, addr, 1, 2, DEVNET));
}

/// A query reserves the length of the last answer, at most the limit, and 64
/// MiB before the first.
#[test]
fn the_reservation_follows_the_last_answer() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    ask(&mut s, addr, 1, 2).unwrap();
    assert_eq!(
        reserved(&s, 1),
        (
            STAGED_LEDGER_FIRST_RESERVATION,
            STAGED_LEDGER_FIRST_RESERVATION
        )
    );
    for (last, reserves) in [(DEVNET, DEVNET), (CAP + 1, CAP), (5_000, 5_000)] {
        s.0.network.staged_ledger_answer_len = Some(last);
        assert_eq!(
            s.0.network.staged_ledger_reservation(&s.0.config.limits),
            reserves
        );
    }
}

/// An answer over the limit is not encoded: the peer gets an empty answer on
/// the same stream, nothing waits, the reservation is released, and the next
/// query reserves the limit.
#[test]
fn an_answer_over_the_limit_gets_none() {
    let parts = staged_ledger(40, 0);
    let len = message(&expected_payload(&parts)).len();
    for (limit, served) in [(len, true), (len - 1, false)] {
        let mut s = state(&[]);
        s.0.config.limits = s.0.config.limits.with_rpc_served_staged_ledger(Some(limit));
        let addr = connection(&mut s);
        let id = ask(&mut s, addr, 1, 2).unwrap();
        let mut reader = Reader::new(2);
        let sent = answer(&mut s, 1, id, parts.clone(), 1);
        assert!(!sent.iter().any(is_reset));
        reader.read(&sent);
        let expected = if served {
            message(&expected_payload(&parts))
        } else {
            message(&none_payload())
        };
        assert_eq!(reader.received, expected, "limit {limit}");
        assert_eq!(reserved(&s, 1), (0, 0));
        assert_eq!(queued(&s), 0);
        assert_eq!(s.0.network.staged_ledger_answer_len, Some(limit));
        assert!(s.0.network.scheduler.connections[&addr].closed.is_none());
        // The next query reserves the answer's length, or the limit.
        ask(&mut s, addr, 1, 4).unwrap();
        assert_eq!(reserved(&s, 1).1, limit);
    }
}

/// An answer larger than the released server's ceiling reaches a reader with
/// the default window whole and unchanged. Each frame fits the reader's
/// window, the queue never holds more than the answer less the window, and
/// it is empty at the end.
#[test]
fn an_answer_over_the_old_ceiling_reaches_a_default_window_reader() {
    let parts = heavy_staged_ledger(200);
    let expected = message(&expected_payload(&parts));
    assert!(expected.len() > OLD_CEILING, "{} bytes", expected.len());
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let id = ask(&mut s, addr, 1, 2).unwrap();
    let mut reader = Reader::new(2);
    let mut sent = answer(&mut s, 1, id, parts, 1);
    assert_eq!(reserved(&s, 1), (0, 0));
    let mut most = queued(&s);
    for t in 1..10_000 {
        assert!(!sent.iter().any(is_reset));
        most = most.max(queued(&s));
        let Some(delta) = reader.read(&sent) else {
            break;
        };
        sent = grant(&mut s, addr, 2, delta, t);
    }
    assert_eq!(reader.received.len(), expected.len());
    assert!(reader.received == expected, "the answer arrived changed");
    assert_eq!(most, expected.len() - WINDOW);
    assert_eq!(queued(&s), 0);
    assert_eq!(s.0.network.staged_ledger_answer_len, Some(expected.len()));
}

/// Queues `len` bytes for peer `n` on a stream with no window left, as a
/// large answer the peer reads slowly.
fn queue(s: &mut State, addr: ConnectionAddr, stream_id: u32, len: usize) -> Vec<P2pAction> {
    if !s.0.network.scheduler.connections[&addr]
        .yamux_state()
        .unwrap()
        .streams
        .contains_key(&stream_id)
    {
        assert!(open(s, addr, stream_id));
    }
    yamux_stream(s, addr, stream_id).window_theirs = 0;
    let mut sent = Vec::new();
    let mut left = len;
    while left > 0 {
        let chunk = left.min(MIB);
        left -= chunk;
        sent.extend(drive(
            s,
            P2pNetworkYamuxAction::OutgoingFrame {
                addr,
                frame: YamuxFrame {
                    flags: YamuxFlags::empty(),
                    stream_id,
                    inner: YamuxFrameInner::Data(vec![0; chunk].into()),
                },
            },
            0,
        ));
    }
    sent
}

/// Once a devnet answer has been built, a staged-ledger query reserves its
/// length, so a second one, from the same peer or another, is refused before
/// any work while the first is pending, with the capacity error OCaml's rate
/// limiter sends. Other queries still fit the reserve.
#[test]
fn a_second_large_query_is_refused_while_the_first_is_pending() {
    let mut s = state(&[]);
    s.0.network.staged_ledger_answer_len = Some(DEVNET);
    let first = connection_for(&mut s, 1);
    ask(&mut s, first, 1, 2).unwrap();
    assert_eq!(reserved(&s, 1), (DEVNET, DEVNET));
    assert!(admitted(&s, first, 1, 2, DEVNET));
    let second = connection_for(&mut s, 2);
    for (addr, n, stream_id) in [(first, 1, 4), (second, 2, 2)] {
        let refused = ask(&mut s, addr, n, stream_id).unwrap_err();
        assert!(refused.iter().any(|action| matches!(
            action,
            P2pAction::Network(P2pNetworkAction::Rpc(P2pNetworkRpcAction::Refusal { .. }))
        )));
        assert!(
            s.0.network.scheduler.rpc_incoming_streams[&peer(n)][&stream_id]
                .pending_incoming
                .is_empty()
        );
    }
    assert_eq!(reserved(&s, 2), (DEVNET, 0));
    // A best-tip query of another peer reserves 4 MiB, within its fair share.
    let third = connection_for(&mut s, 3);
    let mut bytes = Vec::new();
    NeedsLength(()).binprot_write(&mut bytes).unwrap();
    let mut rpc = P2pNetworkRpcState::new(third, 2);
    rpc.is_incoming = true;
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(3))
        .or_default()
        .insert(2, rpc);
    let actions = apply_traced(
        &mut s,
        P2pNetworkRpcAction::IncomingMessage {
            addr: third,
            peer_id: peer(3),
            stream_id: 2,
            message: RpcMessage::Query {
                header: QueryHeader {
                    tag: GetBestTipV2::NAME.into(),
                    version: GetBestTipV2::VERSION,
                    id: 1,
                },
                bytes: bytes.into(),
            },
        },
        0,
    );
    assert!(actions.iter().any(|action| matches!(
        action,
        P2pAction::Channels(P2pChannelsAction::Rpc(
            P2pChannelsRpcAction::RequestReceived { .. }
        ))
    )));
}

/// While one devnet answer waits for peer 1's reader: one more large answer
/// fits neither peer 1 nor another peer, and a staged-ledger query is refused
/// before any work. Small answers to peer 1 and to other peers still fit.
#[test]
fn a_second_large_answer_waits_for_the_first() {
    let mut s = state(&[]);
    let first = connection_for(&mut s, 1);
    assert!(!queue(&mut s, first, 2, DEVNET - WINDOW)
        .iter()
        .any(is_reset));
    let second = connection_for(&mut s, 2);
    assert!(open(&mut s, first, 4));
    assert!(open(&mut s, second, 4));
    let fits = |s: &State, addr, n, len| {
        s.0.network
            .scheduler
            .outgoing_data_admitted(&addr, &peer(n), 4, len, 0, &s.0.config.limits)
    };
    for (addr, n) in [(first, 1), (second, 2)] {
        assert!(!fits(&s, addr, n, DEVNET), "peer {n}");
        assert!(fits(&s, addr, n, MIB), "peer {n}");
    }
    // Even the first answer's reservation doesn't fit beside it now.
    let refused = ask(&mut s, second, 2, 2).unwrap_err();
    assert!(refused.iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Rpc(P2pNetworkRpcAction::Refusal { .. }))
    )));
}

/// Queries admitted together before the first answer reserve 64 MiB each.
/// Their answers then don't fit beside each other's reservations: each gets
/// an empty answer, releasing its reservation, until the last one fits.
#[test]
fn queries_admitted_together_are_answered_one_at_a_time() {
    let mut s = state(&[]);
    let addrs: Vec<_> = (1..=3)
        .map(|n| {
            let addr = connection_for(&mut s, n);
            ask(&mut s, addr, n, 2).unwrap();
            addr
        })
        .collect();
    let fourth = connection_for(&mut s, 4);
    assert!(
        ask(&mut s, fourth, 4, 2).is_err(),
        "192 MiB reserved already"
    );
    for (i, addr) in addrs.iter().enumerate() {
        let n = i as u8 + 1;
        assert_eq!(admitted(&s, *addr, n, 2, DEVNET), n == 3, "peer {n}");
        // Answered empty, or sent: either way its reservation goes.
        s.0.network
            .scheduler
            .rpc_incoming_streams
            .get_mut(&peer(n))
            .unwrap()
            .get_mut(&2)
            .unwrap()
            .pending_incoming
            .clear();
    }
}

/// What a requester gets when the budgets have no room for its answer: an
/// empty answer, with nothing else queued. At a scale small enough to encode.
#[test]
fn an_answer_without_room_gets_none() {
    let parts = staged_ledger(2_000, 0);
    let len = message(&expected_payload(&parts)).len();
    let mut s = state(&[]);
    // A node-wide budget of four answers, and a fair share below one.
    s.0.config.limits =
        s.0.config
            .limits
            .with_yamux_pending_outgoing_total(Some(4 * len))
            .with_yamux_pending_outgoing_reserve(Some(len))
            .with_yamux_pending_outgoing_fair_share(Some(len / 2))
            .with_yamux_pending_outgoing_peer_share(Some(4 * len));
    let first = connection_for(&mut s, 1);
    assert!(!queue(&mut s, first, 2, 2 * len + len / 2)
        .iter()
        .any(is_reset));
    s.0.network.staged_ledger_answer_len = Some(0);
    let second = connection_for(&mut s, 2);
    let id = ask(&mut s, second, 2, 2).unwrap();
    yamux_stream(&mut s, second, 2).window_theirs = 0;
    let sent = answer(&mut s, 2, id, parts, 1);
    assert!(!sent.iter().any(is_reset));
    let none = message(&none_payload());
    assert_eq!(
        s.0.network.scheduler.queued_outgoing_bytes(&peer(2)).1,
        none.len()
    );
    assert_eq!(reserved(&s, 2), (0, 0));
    assert_eq!(s.0.network.staged_ledger_answer_len, Some(len));
    let mut reader = Reader::new(2);
    reader.read(&grant(&mut s, second, 2, WINDOW as u32, 2));
    assert_eq!(reader.received, none);
}

/// A large answer keeps draining while other peers' small answers take the
/// node-wide reserve: the bytes already queued are not checked again when
/// the peer's window opens. Before, a window update offered them to the
/// budgets as new bytes, and the reserve's fair-share rule reset the stream.
#[test]
fn a_large_answer_drains_while_others_take_the_reserve() {
    let mut s = state(&[]);
    let first = connection_for(&mut s, 1);
    assert!(!queue(&mut s, first, 2, DEVNET - WINDOW)
        .iter()
        .any(is_reset));
    for n in 2..=4 {
        let addr = connection_for(&mut s, n);
        assert!(!queue(&mut s, addr, 2, 20 * MIB).iter().any(is_reset));
    }
    let total = queued(&s);
    assert!(total > 192 * MIB, "{total} bytes");
    let sent = grant(&mut s, first, 2, MIB as u32, 1);
    assert!(!sent.iter().any(is_reset));
    let released: usize =
        sent.iter()
            .filter_map(|action| match action {
                P2pAction::Network(P2pNetworkAction::Noise(
                    P2pNetworkNoiseAction::OutgoingData { data, .. },
                )) => Some(data.len() - 12),
                _ => None,
            })
            .sum();
    assert_eq!(released, MIB);
    assert_eq!(queued(&s), total - MIB);
}

/// A reset, a disconnection or a stalled reader releases a large answer's
/// queued bytes.
#[test]
fn a_large_answer_is_released_on_reset_disconnect_and_stall() {
    for end in ["reset", "disconnect", "stall"] {
        let mut s = state(&[]);
        let addr = connection(&mut s);
        s.0.network
            .scheduler
            .rpc_incoming_streams
            .entry(peer(1))
            .or_default()
            .insert(2, P2pNetworkRpcState::new(addr, 2));
        assert!(!queue(&mut s, addr, 2, DEVNET - WINDOW).iter().any(is_reset));
        assert_eq!(queued(&s), DEVNET - WINDOW);
        match end {
            "reset" => apply(
                &mut s,
                P2pNetworkSchedulerAction::StreamReset {
                    addr,
                    peer_id: peer(1),
                    stream_id: 2,
                    protocol: None,
                    reason: P2pNetworkStreamResetReason::RemoteReset,
                },
                1,
            ),
            "disconnect" => apply(&mut s, P2pNetworkSchedulerAction::Prune { addr }, 1),
            _ => {
                assert!(!tick(&mut s, 30).iter().any(is_reset));
                let reset = tick(&mut s, 31)
                    .into_iter()
                    .find(is_reset)
                    .expect("a stalled reader is reset");
                apply(&mut s, reset, 31);
            }
        }
        assert_eq!(queued(&s), 0, "{end}");
    }
}

/// The default limits as the PR states them.
#[test]
fn default_limits() {
    let limits = P2pLimits::default();
    assert_eq!(limits.rpc_served_staged_ledger(), Limit::Some(CAP));
    assert_eq!(
        limits.yamux_pending_outgoing_peer_share(),
        Limit::Some(192 * MIB)
    );
    assert_eq!(limits.rpc_inflight_peer_share(), Limit::Some(192 * MIB));
    assert_eq!(
        limits.yamux_pending_outgoing_total(),
        Limit::Some(256 * MIB)
    );
    assert_eq!(
        limits.yamux_pending_outgoing_reserve(),
        Limit::Some(64 * MIB)
    );
    assert_eq!(
        limits.yamux_pending_outgoing_fair_share(),
        Limit::Some(32 * MIB)
    );
}
