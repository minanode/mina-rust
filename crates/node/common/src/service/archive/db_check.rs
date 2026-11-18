//! The archive database check. OCaml's archive process computes three health
//! numbers from its own database and serves them as Prometheus gauges when
//! it is started with `--metrics-port`. The node reads them over HTTP from
//! `MINA_ARCHIVE_METRICS_ADDRESS`, so it needs no database access or
//! credentials:
//!
//! - `Coda_Archive_max_block_height`: the highest block height stored.
//! - `Coda_Archive_missing_blocks`: heights with no block row in the window
//!   from `max(lowest height, highest - width)` to the highest height. The
//!   width is the archive's `--missing-blocks-width`, 2000 by default.
//! - `Coda_Archive_unparented_blocks`: blocks above the lowest height whose
//!   parent row is absent.
//!
//! The archive refreshes them every two slots, so they lag the database by up
//! to one refresh. The check compares the stored tip with the highest block
//! this node delivered [`behind_after`] earlier: two refreshes.
//!
//! The poller runs on its own thread and never takes the outbox lock while it
//! fetches, so a slow or absent archive can't delay delivery. A failed fetch
//! shows in the status and, rate-limited, in one log line.

use std::{
    collections::VecDeque,
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;

/// Where the archive's metrics are: its `--metrics-port`, for example
/// `http://127.0.0.1:3086`. Optional; without it the check is off.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/archive/cli/archive_cli.ml#L12-L29>
pub const METRICS_ADDRESS_ENV: &str = "MINA_ARCHIVE_METRICS_ADDRESS";

/// Namespace `Coda`, subsystem `Archive`, gauge `max_block_height`. The
/// archive makes each gauge at its first refresh, so a page served before it
/// has none.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_metrics/prometheus_metrics/namespace.ml#L1>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_metrics/prometheus_metrics/mina_metrics.ml#L1852-L1880>
pub const MAX_BLOCK_HEIGHT: &str = "Coda_Archive_max_block_height";
/// Gauge `missing_blocks`.
pub const MISSING_BLOCKS: &str = "Coda_Archive_missing_blocks";
/// Gauge `unparented_blocks`.
pub const UNPARENTED_BLOCKS: &str = "Coda_Archive_unparented_blocks";

/// How often the node reads the gauges. The archive refreshes them every two
/// slots (180 s on Mesa), so a faster poll would only reread old values.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// The whole fetch, connect to last byte.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest metrics page the node reads. A 4.0.4 archive's page is about
/// 2 KiB: the three gauges and one histogram.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The most a log line about the check repeats: one at startup, then one
/// when the verdict changes, at most every 10 minutes.
const LOG_EVERY: Duration = Duration::from_secs(600);

/// How long the stored tip may trail a delivered block: two archive
/// refreshes. A block the archive acknowledged is being written as it
/// answers, and the next refresh, at most `2 x slot` plus the queries' own
/// time later, counts it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/archive/lib/processor.ml#L5129-L5148>
pub fn behind_after(block_window_duration_ms: u64) -> Duration {
    Duration::from_millis(block_window_duration_ms.saturating_mul(4))
}

/// The three gauges, as the archive last computed them. A failed query
/// leaves its gauge at the old value.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/archive_health_queries/archive_health_queries.ml#L3-L69>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/archive/lib/metrics.ml#L34-L126>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ArchiveGauges {
    pub max_block_height: u64,
    pub missing_blocks: u64,
    pub unparented_blocks: u64,
}

/// The check at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// No missing heights, no blocks without a parent, and the stored tip
    /// has every block delivered [`behind_after`] ago.
    Complete,
    /// Missing heights or blocks without a parent.
    Incomplete,
    /// The stored tip trails a block delivered more than [`behind_after`]
    /// ago: acknowledged but not written, or the archive's gauges stopped
    /// refreshing.
    Behind,
    /// No metrics address, or no fetch yet.
    NotChecked,
    /// The last fetch failed, or the page has no archive gauges.
    Unavailable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Behind => "behind",
            Self::NotChecked => "not_checked",
            Self::Unavailable => "unavailable",
        }
    }
}

/// `database` in `GET /archive/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseCheck {
    /// Whether `MINA_ARCHIVE_METRICS_ADDRESS` is set.
    pub configured: bool,
    pub metrics_url: Option<String>,
    pub verdict: Verdict,
    pub last_attempt_at_ms: Option<u64>,
    /// When the gauges below were read.
    pub last_success_at_ms: Option<u64>,
    /// Why the last fetch failed; none after a good one.
    pub last_error: Option<String>,
    pub max_block_height: Option<u64>,
    pub missing_blocks: Option<u64>,
    pub unparented_blocks: Option<u64>,
    /// The highest block the archive acknowledged in this run, when the
    /// gauges were read.
    pub delivered_height: Option<u32>,
    /// `delivered_height - max_block_height`: negative when the database
    /// holds blocks this node didn't send.
    pub stored_tip_lag: Option<i64>,
    /// The highest block delivered at least `behind_after_ms` before the
    /// gauges were read. A stored tip below it is behind.
    pub expected_stored_height: Option<u32>,
    pub behind_after_ms: u64,
    pub poll_interval_ms: u64,
}

impl DatabaseCheck {
    /// The section of a node without `MINA_ARCHIVE_METRICS_ADDRESS`.
    pub fn not_configured() -> Self {
        Self {
            configured: false,
            metrics_url: None,
            verdict: Verdict::NotChecked,
            last_attempt_at_ms: None,
            last_success_at_ms: None,
            last_error: None,
            max_block_height: None,
            missing_blocks: None,
            unparented_blocks: None,
            delivered_height: None,
            stored_tip_lag: None,
            expected_stored_height: None,
            behind_after_ms: 0,
            poll_interval_ms: 0,
        }
    }
}

/// The metrics page's URL from `MINA_ARCHIVE_METRICS_ADDRESS`: an `http` or
/// `https` URL. A bare address gets the archive's path, `/metrics`.
pub fn metrics_url(address: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(address.trim())
        .map_err(|e| format!("{METRICS_ADDRESS_ENV}={address} is not a URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!(
            "{METRICS_ADDRESS_ENV}={address} must be an http:// or https:// URL \
             of the archive's --metrics-port"
        ));
    }
    if url.path() == "/" || url.path().is_empty() {
        url.set_path("/metrics");
    }
    Ok(url)
}

/// [`metrics_url`] of `MINA_ARCHIVE_METRICS_ADDRESS`, when it is set.
pub fn metrics_url_from_env() -> Result<Option<reqwest::Url>, String> {
    match std::env::var(METRICS_ADDRESS_ENV) {
        Ok(address) if !address.trim().is_empty() => metrics_url(&address).map(Some),
        _ => Ok(None),
    }
}

/// The three gauges from a Prometheus text page. Comments and other metrics
/// are skipped. Each gauge must appear once, unlabelled, with a whole,
/// non-negative value.
pub fn parse_metrics(text: &str) -> Result<ArchiveGauges, String> {
    let names = [MAX_BLOCK_HEIGHT, MISSING_BLOCKS, UNPARENTED_BLOCKS];
    let mut values: [Option<u64>; 3] = [None; 3];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name_end = line
            .find(|c: char| c == '{' || c.is_whitespace())
            .unwrap_or(line.len());
        let Some(i) = names.iter().position(|n| *n == &line[..name_end]) else {
            continue;
        };
        let rest = &line[name_end..];
        if rest.starts_with('{') {
            return Err(format!("{} has labels; expected a plain gauge", names[i]));
        }
        let value = rest
            .split_whitespace()
            .next()
            .ok_or_else(|| format!("{} has no value", names[i]))?;
        let number: f64 = value
            .parse()
            .map_err(|_| format!("{} has the value {value:?}, not a number", names[i]))?;
        if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > 9.0e15 {
            return Err(format!(
                "{} has the value {value}, not a whole count",
                names[i]
            ));
        }
        if values[i].replace(number as u64).is_some() {
            return Err(format!("{} appears twice", names[i]));
        }
    }
    match values {
        [Some(max_block_height), Some(missing_blocks), Some(unparented_blocks)] => {
            Ok(ArchiveGauges {
                max_block_height,
                missing_blocks,
                unparented_blocks,
            })
        }
        [None, None, None] => Err("the page has no archive gauges: is this the archive's \
             --metrics-port?"
            .to_owned()),
        _ => {
            let missing: Vec<&str> = names
                .iter()
                .zip(values)
                .filter(|(_, v)| v.is_none())
                .map(|(n, _)| *n)
                .collect();
            Err(format!("the page has no {}", missing.join(" or ")))
        }
    }
}

/// Reads the archive's metrics page.
pub struct MetricsFetcher {
    client: reqwest::blocking::Client,
    url: reqwest::Url,
    timeout: Duration,
    max_body: usize,
}

impl MetricsFetcher {
    pub fn new(url: reqwest::Url, timeout: Duration, max_body: usize) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            url,
            timeout,
            max_body,
        })
    }

    pub fn url(&self) -> &reqwest::Url {
        &self.url
    }

    /// One fetch and parse: at most `timeout` and `max_body` bytes.
    pub fn fetch(&self) -> Result<ArchiveGauges, String> {
        let started = Instant::now();
        let mut response = self
            .client
            .get(self.url.clone())
            .send()
            .map_err(|e| describe(&e, self.timeout))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the metrics page answered HTTP {status}"));
        }
        if response
            .content_length()
            .is_some_and(|len| len > self.max_body as u64)
        {
            return Err(self.too_large());
        }
        let mut body = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if started.elapsed() > self.timeout {
                return Err(self.timed_out());
            }
            let n = match response.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err(self.timed_out()),
                Err(e) => return Err(format!("reading the metrics page failed: {e}")),
            };
            if n == 0 {
                break;
            }
            if body.len() + n > self.max_body {
                return Err(self.too_large());
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let text = String::from_utf8(body).map_err(|_| "the metrics page isn't text".to_owned())?;
        parse_metrics(&text)
    }

    fn timed_out(&self) -> String {
        format!("no full answer within {}", seconds(self.timeout))
    }

    fn too_large(&self) -> String {
        format!(
            "the metrics page is larger than {} KiB",
            self.max_body / 1024
        )
    }
}

/// `5 s`, or `300 ms` below a second.
fn seconds(d: Duration) -> String {
    if d.as_millis().is_multiple_of(1000) {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// A fetch error in a line: the innermost cause, which says what happened
/// ("Connection refused").
fn describe(error: &reqwest::Error, timeout: Duration) -> String {
    if error.is_timeout() {
        return format!("no full answer within {}", seconds(timeout));
    }
    let mut cause: &dyn std::error::Error = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    if error.is_connect() {
        format!("cannot connect: {cause}")
    } else {
        format!("fetch failed: {cause}")
    }
}

/// What the gauges say, given the height the stored tip must reach.
pub fn verdict(gauges: &ArchiveGauges, expected_stored_height: Option<u32>) -> Verdict {
    if gauges.missing_blocks > 0 || gauges.unparented_blocks > 0 {
        Verdict::Incomplete
    } else if expected_stored_height.is_some_and(|h| gauges.max_block_height < u64::from(h)) {
        Verdict::Behind
    } else {
        Verdict::Complete
    }
}

struct State {
    report: DatabaseCheck,
    /// (when, highest delivered height then), oldest first, one per fetch.
    samples: VecDeque<(u64, u32)>,
    last_logged: Option<(u64, Verdict)>,
}

/// The check's state, shared by the poller and `GET /archive/status`.
pub struct DbCheck {
    url: String,
    behind_after: Duration,
    state: Mutex<State>,
}

/// What a fetch found, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLine {
    Unavailable(String),
    Verdict(Verdict, ArchiveGauges),
}

impl DbCheck {
    pub fn new(metrics_url: &reqwest::Url, behind_after: Duration, poll: Duration) -> Self {
        let mut report = DatabaseCheck::not_configured();
        report.configured = true;
        report.metrics_url = Some(metrics_url.to_string());
        report.behind_after_ms = behind_after.as_millis() as u64;
        report.poll_interval_ms = poll.as_millis() as u64;
        Self {
            url: metrics_url.to_string(),
            behind_after,
            state: Mutex::new(State {
                report,
                samples: VecDeque::new(),
                last_logged: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn report(&self) -> DatabaseCheck {
        self.lock().report.clone()
    }

    /// Records a fetch that started at `at_ms`, when the highest delivered
    /// block was `delivered`. Returns the log line to write, if the verdict
    /// changed and the last line is old enough.
    pub fn record(
        &self,
        at_ms: u64,
        delivered: Option<u32>,
        outcome: Result<ArchiveGauges, String>,
    ) -> Option<LogLine> {
        let behind_ms = self.behind_after.as_millis() as u64;
        let mut state = self.lock();
        if let Some(height) = delivered {
            state.samples.push_back((at_ms, height));
        }
        // The newest sample at least `behind_after` old is the reference;
        // older ones are no longer needed.
        let cutoff = at_ms.saturating_sub(behind_ms);
        while state.samples.len() > 1 && state.samples[1].0 <= cutoff {
            state.samples.pop_front();
        }
        let expected = state
            .samples
            .front()
            .filter(|(at, _)| *at <= cutoff)
            .map(|(_, height)| *height);
        let report = &mut state.report;
        report.last_attempt_at_ms = Some(at_ms);
        report.delivered_height = delivered;
        let line = match outcome {
            Ok(gauges) => {
                report.verdict = verdict(&gauges, expected);
                report.last_success_at_ms = Some(at_ms);
                report.last_error = None;
                report.max_block_height = Some(gauges.max_block_height);
                report.missing_blocks = Some(gauges.missing_blocks);
                report.unparented_blocks = Some(gauges.unparented_blocks);
                report.expected_stored_height = expected;
                let stored = i64::try_from(gauges.max_block_height).unwrap_or(i64::MAX);
                report.stored_tip_lag = delivered.map(|h| i64::from(h) - stored);
                LogLine::Verdict(report.verdict, gauges)
            }
            Err(error) => {
                report.verdict = Verdict::Unavailable;
                report.last_error = Some(error.clone());
                LogLine::Unavailable(error)
            }
        };
        let now = state.report.verdict;
        let due = match state.last_logged {
            None => true,
            Some((at, logged)) => {
                logged != now && at_ms.saturating_sub(at) >= LOG_EVERY.as_millis() as u64
            }
        };
        if due {
            // The first line, at startup, doesn't hold back the first change.
            let at = if state.last_logged.is_none() {
                0
            } else {
                at_ms
            };
            state.last_logged = Some((at, now));
            Some(line)
        } else {
            None
        }
    }

    /// One poll: reads the delivered height, fetches, records, logs.
    /// `fetcher` is the client, or why it couldn't be built.
    pub fn poll_once(
        &self,
        fetcher: &Result<MetricsFetcher, String>,
        delivered: impl FnOnce() -> Option<u32>,
    ) {
        let at_ms = now_ms();
        let delivered = delivered();
        let outcome = match fetcher {
            Ok(fetcher) => fetcher.fetch(),
            Err(error) => Err(format!("no HTTP client: {error}")),
        };
        if let Some(line) = self.record(at_ms, delivered, outcome) {
            log(&self.url, &line);
        }
    }
}

fn log(url: &str, line: &LogLine) {
    let url = url.to_owned();
    match line {
        LogLine::Unavailable(error) => mina_node::core::warn!(
            mina_node::core::log::system_time();
            summary = "archive database check unavailable",
            metrics_url = url,
            error = error.clone(),
        ),
        LogLine::Verdict(verdict, gauges) => {
            if *verdict == Verdict::Complete {
                mina_node::core::info!(
                    mina_node::core::log::system_time();
                    summary = "archive database check",
                    verdict = verdict.as_str(),
                    metrics_url = url,
                    max_block_height = gauges.max_block_height,
                    missing_blocks = gauges.missing_blocks,
                    unparented_blocks = gauges.unparented_blocks,
                );
            } else {
                mina_node::core::warn!(
                    mina_node::core::log::system_time();
                    summary = "archive database check",
                    verdict = verdict.as_str(),
                    metrics_url = url,
                    max_block_height = gauges.max_block_height,
                    missing_blocks = gauges.missing_blocks,
                    unparented_blocks = gauges.unparented_blocks,
                );
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Starts the poller on its own thread: a fetch now, then every `poll`.
/// `delivered` gives the highest block the archive acknowledged. The HTTP
/// client is built on that thread: a blocking client must not be made or
/// dropped inside an async runtime, which the archive service runs in.
pub fn spawn(
    url: reqwest::Url,
    behind_after: Duration,
    poll: Duration,
    delivered: impl Fn() -> Option<u32> + Send + 'static,
) -> std::io::Result<Arc<DbCheck>> {
    let check = Arc::new(DbCheck::new(&url, behind_after, poll));
    let polled = check.clone();
    std::thread::Builder::new()
        .name("mina_archive_db_check".to_owned())
        .spawn(move || {
            let fetcher = MetricsFetcher::new(url, FETCH_TIMEOUT, MAX_BODY_BYTES);
            loop {
                polled.poll_once(&fetcher, &delivered);
                std::thread::sleep(poll);
            }
        })?;
    Ok(check)
}

#[cfg(test)]
mod tests;
