//! The archive outbox delivered through the real RPC client to a fake archive
//! process that is down, comes back, and loses a diff it took without
//! answering.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use binprot::{BinProtRead, Nat0};
use mina_p2p_messages::rpc_kernel::MessageHeader;

use super::{
    delivery::{Backoff, SharedOutbox},
    outbox::{BlockRef, Outbox, OutboxConfig},
    RpcSink,
};

const HEADER_MSG: [u8; 7] = [2, 253, 82, 80, 67, 0, 1];
const OK_MSG: [u8; 5] = [2, 1, 0, 1, 0];
const HEARTBEAT_MSG: [u8; 1] = [0];

fn frame(msg: &[u8]) -> Vec<u8> {
    let mut v = (msg.len() as u64).to_le_bytes().to_vec();
    v.extend_from_slice(msg);
    v
}

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 8];
    stream.read_exact(&mut len).ok()?;
    let mut msg = vec![0u8; u64::from_le_bytes(len) as usize];
    stream.read_exact(&mut msg).ok()?;
    Some(msg)
}

/// The query's payload: the bytes of its `NeedsLength` data.
fn query_payload(msg: &[u8]) -> Option<Vec<u8>> {
    let mut r = msg;
    match MessageHeader::binprot_read(&mut r).ok()? {
        MessageHeader::Query(q) => {
            assert_eq!(q.tag.to_string_lossy(), "Send_archive_diff");
            assert_eq!(q.version, 0);
        }
        _ => return None,
    }
    let len = Nat0::binprot_read(&mut r).ok()?.0 as usize;
    assert_eq!(r.len(), len);
    Some(r.to_vec())
}

/// A stand-in for OCaml's archive process: answers the Async RPC handshake,
/// records every `Send_archive_diff` payload it reads and acknowledges it,
/// except that the first `drop_unacked` payloads are read and then the
/// connection is closed without an answer (the archive crashed after taking
/// the diff).
struct FakeArchive {
    received: Arc<Mutex<Vec<Vec<u8>>>>,
    committed: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl FakeArchive {
    fn start(addr: SocketAddr, drop_unacked: usize) -> Self {
        Self::start_with_loss(addr, drop_unacked, 0)
    }

    fn start_with_loss(addr: SocketAddr, drop_unacked: usize, drop_acked: usize) -> Self {
        let listener = TcpListener::bind(addr).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(AtomicUsize::new(0));
        let committed = Arc::new(Mutex::new(Vec::new()));
        let stored = committed.clone();
        let rec = received.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let rec = rec.clone();
                let dropped = dropped.clone();
                let stored = stored.clone();
                thread::spawn(move || {
                    let mut hello = frame(&HEADER_MSG);
                    hello.extend(frame(&HEARTBEAT_MSG));
                    if stream.write_all(&hello).is_err() {
                        return;
                    }
                    while let Some(msg) = read_frame(&mut stream) {
                        if msg == HEADER_MSG || msg == HEARTBEAT_MSG {
                            continue;
                        }
                        let Some(payload) = query_payload(&msg) else {
                            continue;
                        };
                        rec.lock().unwrap().push(payload.clone());
                        let attempt = dropped.fetch_add(1, Ordering::SeqCst);
                        if attempt < drop_unacked {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                        if stream.write_all(&frame(&OK_MSG)).is_err() {
                            return;
                        }
                        // Like processor.ml, acknowledge before the transaction.
                        // The configured first attempts fail after acceptance.
                        if attempt >= drop_unacked + drop_acked {
                            let mut stored = stored.lock().unwrap();
                            if !stored.contains(&payload) {
                                stored.push(payload);
                            }
                        }
                    }
                });
            }
        });
        Self {
            received,
            committed,
        }
    }

    fn received(&self) -> Vec<Vec<u8>> {
        self.received.lock().unwrap().clone()
    }
}

fn free_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn block(height: u32) -> BlockRef {
    BlockRef {
        height,
        state_hash: format!("3NHash{height}"),
    }
}

fn payload(height: u32) -> Vec<u8> {
    format!("diff of block {height}").into_bytes()
}

fn append(outbox: &SharedOutbox, height: u32) {
    outbox
        .append(
            block(height),
            &format!("3NHash{}", height - 1),
            &payload(height),
        )
        .unwrap();
}

fn fast_backoff() -> Backoff {
    Backoff {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
    }
}

fn start(dir: &tempfile::TempDir, addr: SocketAddr) -> (SharedOutbox, thread::JoinHandle<()>) {
    let outbox = SharedOutbox::new(
        Outbox::open(OutboxConfig::new(dir.path())).unwrap(),
        Some(addr.to_string()),
    );
    let handle = outbox.spawn_delivery(
        Box::new(RpcSink {
            address: addr,
            timeout: Duration::from_secs(5),
        }),
        fast_backoff(),
    );
    (outbox, handle)
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Each payload's first arrival, in arrival order.
fn first_arrivals(received: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut seen = Vec::new();
    for p in received {
        if !seen.contains(p) {
            seen.push(p.clone());
        }
    }
    seen
}

#[test]
fn backoff_doubles_up_to_its_cap() {
    let b = Backoff::default();
    let secs: Vec<_> = [1, 2, 3, 4, 6, 7, 8, 100]
        .iter()
        .map(|n| b.delay(*n).as_secs())
        .collect();
    assert_eq!(secs, vec![1, 2, 4, 8, 32, 60, 60, 60]);
}

#[test]
fn archive_down_then_back_receives_every_block_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let addr = free_addr();
    let (outbox, handle) = start(&dir, addr);

    // The archive is down: nothing is lost, delivery keeps failing.
    for h in 1..=5 {
        append(&outbox, h);
    }
    wait_for("failures", Duration::from_secs(5), || {
        outbox.status().delivery.consecutive_failures >= 3
    });
    let status = outbox.status();
    assert_eq!(status.outbox.pending_entries, 5);
    assert_eq!(status.delivery.stuck_on, Some(block(1)));
    assert!(status.delivery.last_error.is_some());

    // It comes back, takes block 1 and crashes before answering, then
    // recovers.
    let archive = FakeArchive::start(addr, 1);
    for h in 6..=10 {
        append(&outbox, h);
    }
    assert!(outbox.wait_empty(Duration::from_secs(20)));

    let received = archive.received();
    let expected: Vec<_> = (1..=10).map(payload).collect();
    assert_eq!(first_arrivals(&received), expected);
    // The unanswered diff came again; everything else exactly once.
    assert_eq!(received.len(), 11);
    assert_eq!(&received[..2], &[payload(1), payload(1)]);
    let status = outbox.status();
    assert_eq!(status.delivery.consecutive_failures, 0);
    assert_eq!(status.outbox.last_delivered, Some(block(10)));
    assert_eq!(status.outbox.gaps_total, 0);

    outbox.stop();
    handle.join().unwrap();
}

#[test]
fn a_node_restart_resumes_from_the_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let addr = free_addr();

    // Archive down; the node queues four blocks and stops.
    let (outbox, handle) = start(&dir, addr);
    for h in 1..=4 {
        append(&outbox, h);
    }
    wait_for("a failure", Duration::from_secs(5), || {
        outbox.status().delivery.consecutive_failures >= 1
    });
    outbox.stop();
    handle.join().unwrap();
    drop(outbox);

    // The archive comes back while the node restarts. The restarted node
    // re-applies its frontier (blocks 3 and 4 again) and goes on.
    let archive = FakeArchive::start(addr, 0);
    let (outbox, handle) = start(&dir, addr);
    for h in 3..=6 {
        append(&outbox, h);
    }
    assert!(outbox.wait_empty(Duration::from_secs(20)));

    let expected: Vec<_> = (1..=6).map(payload).collect();
    assert_eq!(archive.received(), expected);
    assert_eq!(outbox.status().outbox.gaps_total, 0);

    // A replay after everything was delivered re-sends the frontier, which
    // the archive absorbs by state hash.
    append(&outbox, 6);
    assert!(outbox.wait_empty(Duration::from_secs(20)));
    assert_eq!(archive.received().len(), 7);

    outbox.stop();
    handle.join().unwrap();
}

#[test]
fn an_archive_that_never_answers_times_out_and_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let addr = free_addr();
    // Accepts connections and never says anything.
    let listener = TcpListener::bind(addr).unwrap();
    let silent = thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().take(2) {
            held.push(stream);
        }
        thread::sleep(Duration::from_secs(2));
    });
    let outbox = SharedOutbox::new(Outbox::open(OutboxConfig::new(dir.path())).unwrap(), None);
    let handle = outbox.spawn_delivery(
        Box::new(RpcSink {
            address: addr,
            timeout: Duration::from_millis(200),
        }),
        fast_backoff(),
    );
    append(&outbox, 1);
    wait_for("two timeouts", Duration::from_secs(5), || {
        outbox.status().delivery.consecutive_failures >= 2
    });
    assert_eq!(outbox.pending_len(), 1);
    assert!(outbox
        .status()
        .delivery
        .last_error
        .unwrap()
        .contains("did not answer"));
    outbox.stop();
    handle.join().unwrap();
    silent.join().unwrap();
}

#[test]
fn acknowledged_but_not_stored_is_repaired_beyond_the_frontier() {
    let dir = tempfile::tempdir().unwrap();
    let addr = free_addr();
    let archive = FakeArchive::start_with_loss(addr, 0, 1);
    let (outbox, handle) = start(&dir, addr);
    append(&outbox, 10);
    assert!(outbox.wait_empty(Duration::from_secs(10)));
    assert_eq!(outbox.status().outbox.retained_entries, 1);
    assert!(!archive.committed.lock().unwrap().contains(&payload(10)));
    // This accepted height puts block 10 beyond a k=290 frontier, within the
    // retained 3k window. No reapplication of block 10 supplies its diff.
    append(&outbox, 400);
    assert!(outbox.wait_empty(Duration::from_secs(10)));
    assert_eq!(outbox.status().outbox.retained_entries, 2);
    assert_eq!(outbox.resend_retained().unwrap(), 2);
    assert!(outbox.wait_empty(Duration::from_secs(10)));
    wait_for("repaired database", Duration::from_secs(5), || {
        archive.committed.lock().unwrap().contains(&payload(10))
    });
    outbox.stop();
    handle.join().unwrap();
    drop(outbox);

    // Model the archive losing that acknowledged transaction again. Startup
    // automatically replays the retained window without frontier assistance.
    archive
        .committed
        .lock()
        .unwrap()
        .retain(|p| *p != payload(10));
    let (outbox, handle) = start(&dir, addr);
    assert!(outbox.wait_empty(Duration::from_secs(10)));
    wait_for("startup repair", Duration::from_secs(5), || {
        archive.committed.lock().unwrap().contains(&payload(10))
    });
    assert_eq!(outbox.status().outbox.retained_entries, 2);
    assert_eq!(
        archive.committed.lock().unwrap().len(),
        2,
        "idempotent duplicates"
    );
    outbox.stop();
    handle.join().unwrap();
}
