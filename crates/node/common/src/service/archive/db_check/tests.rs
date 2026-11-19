//! The database check: parsing a real 4.0.4 archive's metrics page, the
//! verdict and its reference height, fetches from a fake metrics server, and
//! the status section of the outbox.
//!
//! The two fixtures are `GET /metrics` from the pinned mina-archive 4.0.4
//! (c9e2eac5) started with `--metrics-port`, fed by this node on devnet, on
//! 2026-11-18. `complete` was captured with 292 blocks stored, heights
//! 591352 to 591643. `incomplete` was captured at the archive's next refresh
//! after the block at height 591500 was deleted and its child unlinked. The
//! expected values are the archive's own, and the archive's SQL run by hand on
//! the same database gave the same numbers.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use super::{
    super::{
        delivery::{ArchiveSink, SharedOutbox},
        outbox::{BlockRef, Outbox, OutboxConfig},
    },
    *,
};

const COMPLETE: &str = include_str!("metrics-4.0.4-complete.txt");
const INCOMPLETE: &str = include_str!("metrics-4.0.4-incomplete.txt");

#[test]
fn parses_a_404_archive_page() {
    assert_eq!(
        parse_metrics(COMPLETE),
        Ok(ArchiveGauges {
            max_block_height: 591643,
            missing_blocks: 0,
            unparented_blocks: 0,
        })
    );
    assert_eq!(
        parse_metrics(INCOMPLETE),
        Ok(ArchiveGauges {
            max_block_height: 591643,
            missing_blocks: 1,
            unparented_blocks: 1,
        })
    );
}

#[test]
fn parses_other_prometheus_spellings() {
    // Standard comments, a timestamp, CRLF line ends and float values.
    let page = "# HELP Coda_Archive_max_block_height x\r\n\
                # TYPE Coda_Archive_max_block_height gauge\r\n\
                Coda_Archive_max_block_height 12.0 1700000000000\r\n\
                Coda_Archive_missing_blocks\t0\r\n\
                Coda_Archive_unparented_blocks 3e0\r\n\
                Coda_Archive_max_block_height_other 99\r\n";
    assert_eq!(
        parse_metrics(page),
        Ok(ArchiveGauges {
            max_block_height: 12,
            missing_blocks: 0,
            unparented_blocks: 3,
        })
    );
}

fn page_with(max: &str) -> String {
    format!("Coda_Archive_max_block_height {max}\nCoda_Archive_missing_blocks 0\nCoda_Archive_unparented_blocks 0\n")
}

#[test]
fn refuses_values_that_are_not_counts() {
    for bad in ["NaN", "+Inf", "-1", "1.5", "abc", "1e300"] {
        let error = parse_metrics(&page_with(bad)).unwrap_err();
        assert!(error.contains(MAX_BLOCK_HEIGHT), "{bad}: {error}");
    }
    assert!(parse_metrics("Coda_Archive_max_block_height\n")
        .unwrap_err()
        .contains("no value"));
    let labelled = "Coda_Archive_max_block_height{a=\"b\"} 1\n";
    assert!(parse_metrics(labelled).unwrap_err().contains("labels"));
    let twice = format!("{}Coda_Archive_missing_blocks 2\n", page_with("5"));
    assert_eq!(
        parse_metrics(&twice).unwrap_err(),
        "Coda_Archive_missing_blocks appears twice"
    );
}

#[test]
fn names_the_missing_gauges() {
    assert_eq!(
        parse_metrics("Coda_Archive_max_block_height 5\n").unwrap_err(),
        "the page has no Coda_Archive_missing_blocks or Coda_Archive_unparented_blocks"
    );
    // A daemon's metrics, or anything else.
    let error = parse_metrics("Coda_Runtime_rss_bytes 1\nhello world\n").unwrap_err();
    assert!(error.contains("no archive gauges"), "{error}");
    assert!(parse_metrics("").unwrap_err().contains("no archive gauges"));
}

#[test]
fn metrics_url_defaults_the_path() {
    let url = |s| metrics_url(s).map(|u| u.to_string());
    assert_eq!(
        url("http://127.0.0.1:3086"),
        Ok("http://127.0.0.1:3086/metrics".to_owned())
    );
    assert_eq!(
        url(" http://archive:3086/ "),
        Ok("http://archive:3086/metrics".to_owned())
    );
    assert_eq!(
        url("https://proxy.example/archive/metrics"),
        Ok("https://proxy.example/archive/metrics".to_owned())
    );
    assert!(url("127.0.0.1:3086").is_err());
    assert!(url("ftp://127.0.0.1/metrics").is_err());
    assert!(url("not a url").is_err());
}

#[test]
fn behind_after_is_two_archive_refreshes() {
    // Mesa slots are 90 s and the archive refreshes every two slots.
    assert_eq!(behind_after(90_000), Duration::from_secs(360));
}

fn gauges(max: u64, missing: u64, unparented: u64) -> ArchiveGauges {
    ArchiveGauges {
        max_block_height: max,
        missing_blocks: missing,
        unparented_blocks: unparented,
    }
}

#[test]
fn verdicts() {
    assert_eq!(verdict(&gauges(100, 0, 0), None), Verdict::Complete);
    assert_eq!(verdict(&gauges(100, 0, 0), Some(100)), Verdict::Complete);
    assert_eq!(verdict(&gauges(99, 0, 0), Some(100)), Verdict::Behind);
    // Missing blocks say more than a late tip: they take precedence.
    assert_eq!(verdict(&gauges(99, 1, 0), Some(100)), Verdict::Incomplete);
    assert_eq!(verdict(&gauges(100, 0, 2), None), Verdict::Incomplete);
}

const MIN: u64 = 60_000;

fn check() -> DbCheck {
    let url = metrics_url("http://127.0.0.1:3086").unwrap();
    DbCheck::new(&url, Duration::from_secs(360), Duration::from_secs(60))
}

#[test]
fn a_new_check_is_not_checked_yet() {
    let report = check().report();
    assert!(report.configured);
    assert_eq!(report.verdict, Verdict::NotChecked);
    assert_eq!(
        report.metrics_url.as_deref(),
        Some("http://127.0.0.1:3086/metrics")
    );
    assert_eq!(report.behind_after_ms, 360_000);
    assert_eq!(report.poll_interval_ms, 60_000);
    let off = DatabaseCheck::not_configured();
    assert!(!off.configured);
    assert_eq!(off.verdict, Verdict::NotChecked);
}

#[test]
fn the_stored_tip_must_reach_what_was_delivered_two_refreshes_ago() {
    let c = check();
    let t0 = 1_000 * MIN;
    // Delivered 100 at t0; the archive's gauge still says 90 for a while.
    c.record(t0, Some(100), Ok(gauges(90, 0, 0)));
    let r = c.report();
    assert_eq!(r.verdict, Verdict::Complete);
    assert_eq!(r.expected_stored_height, None);
    assert_eq!(r.stored_tip_lag, Some(10));
    c.record(t0 + 5 * MIN, Some(104), Ok(gauges(90, 0, 0)));
    assert_eq!(c.report().verdict, Verdict::Complete);
    // Six minutes after 100 was delivered, the tip must have it.
    c.record(t0 + 6 * MIN, Some(105), Ok(gauges(99, 0, 0)));
    let r = c.report();
    assert_eq!(r.verdict, Verdict::Behind);
    assert_eq!(r.expected_stored_height, Some(100));
    assert_eq!(r.delivered_height, Some(105));
    assert_eq!(r.stored_tip_lag, Some(6));
    c.record(t0 + 7 * MIN, Some(105), Ok(gauges(104, 0, 0)));
    assert_eq!(c.report().verdict, Verdict::Complete);
    // Blocks this node didn't send are fine.
    c.record(t0 + 8 * MIN, Some(105), Ok(gauges(110, 0, 0)));
    let r = c.report();
    assert_eq!(r.verdict, Verdict::Complete);
    assert_eq!(r.stored_tip_lag, Some(-5));
}

#[test]
fn a_replay_of_old_blocks_does_not_lower_the_reference() {
    // The reference is the highest height delivered, whatever was sent
    // last; the outbox keeps it (see the delivery test below), and the
    // samples keep only what they were given.
    let c = check();
    let t0 = 1_000 * MIN;
    c.record(t0, Some(200), Ok(gauges(200, 0, 0)));
    c.record(t0 + 7 * MIN, Some(200), Ok(gauges(199, 0, 0)));
    assert_eq!(c.report().verdict, Verdict::Behind);
}

#[test]
fn a_failed_fetch_is_unavailable_and_keeps_the_last_values() {
    let c = check();
    c.record(MIN, Some(10), Ok(gauges(10, 0, 0)));
    c.record(2 * MIN, Some(11), Err("cannot connect: refused".to_owned()));
    let r = c.report();
    assert_eq!(r.verdict, Verdict::Unavailable);
    assert_eq!(r.last_error.as_deref(), Some("cannot connect: refused"));
    assert_eq!(r.last_attempt_at_ms, Some(2 * MIN));
    assert_eq!(r.last_success_at_ms, Some(MIN));
    assert_eq!(r.max_block_height, Some(10));
    c.record(3 * MIN, Some(11), Ok(gauges(11, 0, 0)));
    let r = c.report();
    assert_eq!(r.verdict, Verdict::Complete);
    assert_eq!(r.last_error, None);
}

#[test]
fn logs_only_changes_and_at_most_every_ten_minutes() {
    let c = check();
    let t = |m: u64| 1_000 * MIN + m * MIN;
    let error = || Err::<ArchiveGauges, _>("down".to_owned());
    assert!(matches!(
        c.record(t(0), None, error()),
        Some(LogLine::Unavailable(_))
    ));
    // Still down: nothing more, however long.
    assert_eq!(c.record(t(1), None, error()), None);
    assert_eq!(c.record(t(30), None, error()), None);
    // Back up after 30 minutes: one line.
    assert_eq!(
        c.record(t(31), None, Ok(gauges(5, 0, 0))),
        Some(LogLine::Verdict(Verdict::Complete, gauges(5, 0, 0)))
    );
    // Flapping within ten minutes of that line: quiet.
    assert_eq!(c.record(t(32), None, error()), None);
    assert_eq!(c.record(t(33), None, Ok(gauges(5, 0, 0))), None);
    assert_eq!(c.record(t(34), None, Ok(gauges(5, 1, 0))), None);
    // Still incomplete ten minutes on: logged then.
    assert_eq!(
        c.record(t(41), None, Ok(gauges(5, 1, 0))),
        Some(LogLine::Verdict(Verdict::Incomplete, gauges(5, 1, 0)))
    );
}

#[test]
fn the_startup_line_does_not_hold_back_the_first_change() {
    let c = check();
    let t = |m: u64| 1_000 * MIN + m * MIN;
    assert!(c.record(t(0), None, Ok(gauges(5, 0, 0))).is_some());
    // Six minutes after startup a block goes missing: logged at once.
    assert_eq!(
        c.record(t(6), None, Ok(gauges(5, 1, 1))),
        Some(LogLine::Verdict(Verdict::Incomplete, gauges(5, 1, 1)))
    );
    // Repaired a few minutes later: within ten minutes of that line.
    assert_eq!(c.record(t(9), None, Ok(gauges(5, 0, 0))), None);
    assert_eq!(
        c.record(t(16), None, Ok(gauges(5, 0, 0))),
        Some(LogLine::Verdict(Verdict::Complete, gauges(5, 0, 0)))
    );
}

/// A one-request-at-a-time HTTP server on a free port; `answer` writes the
/// whole response for each request.
fn serve(answer: impl Fn(&mut TcpStream) + Send + 'static) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ => break,
                }
            }
            answer(&mut stream);
        }
    });
    address
}

fn ok_text(body: &'static str) -> impl Fn(&mut TcpStream) + Send + 'static {
    move |s: &mut TcpStream| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
}

fn fetcher(address: SocketAddr, timeout: Duration, max_body: usize) -> MetricsFetcher {
    let url = metrics_url(&format!("http://{address}")).unwrap();
    MetricsFetcher::new(url, timeout, max_body).unwrap()
}

#[test]
fn fetches_the_archive_page() {
    let address = serve(ok_text(INCOMPLETE));
    let got = fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES).fetch();
    assert_eq!(got, Ok(gauges(591643, 1, 1)));
}

#[test]
fn an_archive_that_never_answers_times_out() {
    let address = serve(|_| thread::sleep(Duration::from_secs(10)));
    let started = Instant::now();
    let error = fetcher(address, Duration::from_millis(300), MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3), "{error}");
    assert!(error.contains("no full answer within"), "{error}");
}

#[test]
fn a_page_that_trickles_in_times_out() {
    // Headers at once, then a byte every 100 ms: each read is quick, the
    // whole is not.
    let address = serve(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n"
        );
        for _ in 0..1000 {
            if s.write_all(b"#").is_err() {
                return;
            }
            let _ = s.flush();
            thread::sleep(Duration::from_millis(100));
        }
    });
    let started = Instant::now();
    let error = fetcher(address, Duration::from_millis(800), MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(4), "{error}");
    assert!(error.contains("no full answer within"), "{error}");
}

/// Within the deadline plus scheduling slack, well short of the server's
/// next step: a limit that restarted per step would end later.
fn assert_within_deadline(started: Instant, deadline: Duration) {
    let elapsed = started.elapsed();
    assert!(
        elapsed < deadline + Duration::from_millis(400),
        "the fetch took {elapsed:?} against a {deadline:?} deadline"
    );
}

#[test]
fn late_headers_then_a_late_body_end_at_the_deadline() {
    // Headers at 0.8 of the deadline, the body at 1.6: each wait alone is
    // shorter than the deadline, the whole is not.
    let address = serve(|s| {
        thread::sleep(Duration::from_millis(800));
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n"
        );
        let _ = s.flush();
        thread::sleep(Duration::from_millis(800));
        let _ = s.write_all(b"#");
    });
    let deadline = Duration::from_secs(1);
    let started = Instant::now();
    let error = fetcher(address, deadline, MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert_within_deadline(started, deadline);
    assert!(error.contains("no full answer within"), "{error}");
}

#[test]
fn a_full_page_whose_connection_closes_late_is_refused() {
    // The whole page at 0.6 of the deadline without a Content-Length, so the
    // body ends only when the connection closes, at 1.6.
    let address = serve(|s| {
        thread::sleep(Duration::from_millis(600));
        let _ = write!(s, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{COMPLETE}");
        let _ = s.flush();
        thread::sleep(Duration::from_secs(1));
    });
    let deadline = Duration::from_secs(1);
    let started = Instant::now();
    let error = fetcher(address, deadline, MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert_within_deadline(started, deadline);
    assert!(error.contains("no full answer within"), "{error}");
}

#[test]
fn an_oversized_page_is_refused() {
    // Declared too large.
    let address = serve(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: 5000000\r\nConnection: close\r\n\r\n"
        );
    });
    let error = fetcher(address, FETCH_TIMEOUT, 4096).fetch().unwrap_err();
    assert_eq!(error, "the metrics page is larger than 4 KiB");
    // Undeclared, chunked, and too large once read.
    let address = serve(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        );
        let chunk = "#".repeat(1000);
        for _ in 0..10 {
            let _ = write!(s, "{:x}\r\n{chunk}\r\n", chunk.len());
        }
        let _ = write!(s, "0\r\n\r\n");
    });
    let error = fetcher(address, FETCH_TIMEOUT, 4096).fetch().unwrap_err();
    assert_eq!(error, "the metrics page is larger than 4 KiB");
}

#[test]
fn garbage_and_missing_gauges() {
    let address = serve(|s| {
        let body = [0xffu8, 0xfe, 0x00, 0x80];
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = s.write_all(&body);
    });
    assert_eq!(
        fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES).fetch(),
        Err("the metrics page isn't text".to_owned())
    );
    let address = serve(ok_text("<html>not metrics</html>\n"));
    let error = fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert!(error.contains("no archive gauges"), "{error}");
    let address = serve(ok_text(
        "Coda_Archive_max_block_height 7\nCoda_Archive_missing_blocks 0\n",
    ));
    assert_eq!(
        fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES).fetch(),
        Err("the page has no Coda_Archive_unparented_blocks".to_owned())
    );
}

#[test]
fn an_archive_that_is_down_or_answers_an_error() {
    // A port nothing listens on.
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let error = fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES)
        .fetch()
        .unwrap_err();
    assert!(error.starts_with("cannot connect: "), "{error}");
    // The archive's own answer to another path, and a redirect, which is
    // not followed.
    let address = serve(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\nConnection: close\r\n\r\nBad request"
        );
    });
    assert_eq!(
        fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES).fetch(),
        Err("the metrics page answered HTTP 400 Bad Request".to_owned())
    );
    let address = serve(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 301 Moved Permanently\r\nLocation: http://192.0.2.1/\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
        );
    });
    assert_eq!(
        fetcher(address, FETCH_TIMEOUT, MAX_BODY_BYTES).fetch(),
        Err("the metrics page answered HTTP 301 Moved Permanently".to_owned())
    );
}

struct Accept;

impl ArchiveSink for Accept {
    fn deliver(&mut self, _payload: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

fn block(height: u32) -> BlockRef {
    BlockRef {
        height,
        state_hash: format!("3NHash{height}"),
    }
}

#[test]
fn the_status_reports_the_check_and_the_highest_delivered_block() {
    let dir = tempfile::tempdir().unwrap();
    let outbox = SharedOutbox::new(Outbox::open(OutboxConfig::new(dir.path())).unwrap(), None);
    assert_eq!(outbox.status().database, DatabaseCheck::not_configured());
    assert_eq!(outbox.highest_delivered_height(), None);
    for height in [10, 11] {
        outbox
            .append(block(height), &format!("3NHash{}", height - 1), b"diff")
            .unwrap();
        outbox.deliver_one(&mut Accept).unwrap();
    }
    assert_eq!(outbox.highest_delivered_height(), Some(11));
    // Replaying block 10 makes it the last delivered, not the highest.
    outbox.resend_block("3NHash10").unwrap();
    outbox.deliver_one(&mut Accept).unwrap();
    let status = outbox.status();
    assert_eq!(status.outbox.last_delivered.map(|b| b.height), Some(10));
    assert_eq!(
        status.delivery.highest_delivered.map(|b| b.height),
        Some(11)
    );
    assert_eq!(outbox.highest_delivered_height(), Some(11));

    let check = Arc::new(check());
    check.record(MIN, outbox.highest_delivered_height(), Ok(gauges(11, 0, 0)));
    outbox.set_database_check(check);
    let database = outbox.status().database;
    assert!(database.configured);
    assert_eq!(database.verdict, Verdict::Complete);
    assert_eq!(database.max_block_height, Some(11));
    assert_eq!(database.delivered_height, Some(11));
    let json = serde_json::to_value(outbox.status()).unwrap();
    assert_eq!(json["database"]["verdict"], "complete");
    assert_eq!(json["highest_delivered"]["height"], 11);
}

#[test]
fn the_poller_reads_the_page_on_its_own_thread() {
    let address = serve(ok_text(COMPLETE));
    let url = metrics_url(&format!("http://{address}")).unwrap();
    let check = spawn(
        url,
        Duration::from_secs(360),
        Duration::from_millis(50),
        || Some(591643),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while check.report().last_success_at_ms.is_none() {
        assert!(Instant::now() < deadline, "no fetch: {:?}", check.report());
        thread::sleep(Duration::from_millis(20));
    }
    let report = check.report();
    assert_eq!(report.verdict, Verdict::Complete);
    assert_eq!(report.max_block_height, Some(591643));
    assert_eq!(report.stored_tip_lag, Some(0));
}
