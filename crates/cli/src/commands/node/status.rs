//! Read-only operator status. Never fetch /state: it is large and its JSONPath
//! evaluator on older nodes can panic for otherwise valid expressions.
use std::{
    io::Read,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, clap::Args)]
pub struct Status {
    /// The running node's HTTP base URL. The report names the network that
    /// node is on.
    #[arg(
        long,
        alias = "url",
        default_value = "http://127.0.0.1:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: Url,
    /// Use only GET /status; omit build and verification statistics.
    #[arg(long)]
    pub status_only: bool,
    /// Print a JSON report, also when a request fails.
    #[arg(long)]
    pub json: bool,
    /// Exit with status 1 if the node is not synced, its best tip is missing
    /// or older than `--max-tip-age`, or it has fewer peers than
    /// `--min-peers`.
    #[arg(long)]
    pub check: bool,
    /// The oldest best tip `--check` accepts, in seconds.
    #[arg(long, default_value_t = 1800, value_name = "SECONDS")]
    pub max_tip_age: u64,
    /// The fewest connected peers `--check` accepts; peers still connecting
    /// don't count.
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub min_peers: usize,
    /// Seconds to wait for each HTTP request (1 to 300).
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=300), value_name = "SECONDS")]
    pub timeout: u64,
}

/// The node's URL as a base that routes join onto: no query or fragment, and
/// its path ending in exactly one `/`, so `http://x/prefix/` and
/// `http://x/prefix` both give `/prefix/status`.
pub(crate) fn base_url(node: &Url) -> Url {
    let mut base = node.clone();
    base.set_query(None);
    base.set_fragment(None);
    base.set_path(&format!("{}/", base.path().trim_end_matches('/')));
    base
}

#[derive(Debug, Deserialize)]
struct Response {
    network: Option<String>,
    chain_id: Option<String>,
    uptime_secs: Option<u64>,
    transition_frontier: Frontier,
    peers: Vec<Peer>,
    #[serde(default)]
    resources_status: Value,
}

#[derive(Debug, Deserialize)]
struct Frontier {
    best_tip: Option<Tip>,
    root: Option<Tip>,
    sync: Sync,
}

#[derive(Debug, Deserialize, Serialize)]
struct Tip {
    hash: String,
    height: u64,
    timestamp_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Sync {
    status: String,
    phase: String,
}

#[derive(Debug, Deserialize)]
struct Peer {
    connection_status: String,
    is_libp2p: bool,
    best_tip: Option<String>,
    best_tip_timestamp: Option<u64>, // RPC Timestamp is Unix nanoseconds.
}

#[derive(Debug, Serialize)]
struct PeerCounts {
    connected: usize,
    libp2p: usize,
    webrtc: usize,
}

#[derive(Debug, Serialize)]
struct Verification {
    /// Action statistics since the latest best-tip change, not lifetime totals.
    since_unix_ns: u64,
    block_height: u64,
    block_hash: String,
    block_success: Option<u64>,
    block_failures: Option<u64>,
    work_success: Option<u64>,
    work_failures: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Summary {
    network: Option<String>,
    chain_id: Option<String>,
    sync_status: String,
    sync_phase: String,
    best_tip: Option<Tip>,
    tip_age_secs: Option<u64>,
    tip_timestamp_source: Option<&'static str>,
    root_height: Option<u64>,
    root_height_source: Option<&'static str>,
    peers: PeerCounts,
    build_commit: Option<String>,
    uptime_secs: Option<u64>,
    /// Allocation estimate for P2P only, NOT process RSS or total memory.
    p2p_memory_bytes: Option<u64>,
    verification: Option<Verification>,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u8,
    observed_at_unix_ms: u64,
    status: Option<Summary>,
    healthy: bool,
    reasons: Vec<String>,
    warnings: Vec<String>,
}

impl Status {
    pub fn run(self) -> anyhow::Result<()> {
        let (report, failed) = self.collect();
        if self.json {
            println!("{}", serde_json::to_string(&report)?);
        } else if self.check || failed {
            println!("{}", report.health_line());
        } else {
            print!("{}", report.human());
        }
        if failed || (self.check && !report.healthy) {
            std::process::exit(1);
        }
        Ok(())
    }

    fn collect(&self) -> (Report, bool) {
        let mut report = Report {
            schema_version: 1,
            observed_at_unix_ms: now_ms(),
            status: None,
            healthy: false,
            reasons: vec![],
            warnings: vec![],
        };
        match self.fetch(&mut report.warnings) {
            Ok(summary) => {
                report.observed_at_unix_ms = now_ms();
                report.reasons = summary.reasons(self.max_tip_age, self.min_peers);
                report.healthy = report.reasons.is_empty();
                report.status = Some(summary);
                (report, false)
            }
            Err(error) => {
                report.reasons.push(one_line(&format!("{error:#}")));
                (report, true)
            }
        }
    }

    fn fetch(&self, warnings: &mut Vec<String>) -> anyhow::Result<Summary> {
        if !matches!(self.node.scheme(), "http" | "https") {
            bail!("--node must use http or https");
        }
        if !self.node.username().is_empty() || self.node.password().is_some() {
            bail!("--node must not contain credentials");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout))
            .redirect(Policy::none())
            .build()?;
        let base = base_url(&self.node);
        let get = |path: &str| -> anyhow::Result<Value> {
            let url = base.join(path)?;
            let response = client.get(url).send().map_err(|e| e.without_url())?;
            if !response.status().is_success() {
                bail!("HTTP {}", response.status());
            }
            // Bound both memory use and time for an unexpected endpoint.
            let mut bytes = Vec::new();
            response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 4 * 1024 * 1024 {
                bail!("response exceeds 4 MiB");
            }
            Ok(serde_json::from_slice(&bytes)?)
        };
        let raw = get("status").context("GET /status")?;
        let response: Response = serde_json::from_value(raw).context("invalid /status response")?;
        if self.status_only {
            warnings.push("build and verification statistics not requested (--status-only)".into());
            return Ok(Summary::from_response(response, now_ms()));
        }
        let mut optional = |path: &str| match get(path) {
            Ok(value) => Some(value),
            Err(error) => {
                warnings.push(one_line(&format!("GET /{path}: {error:#}")));
                None
            }
        };
        let build = optional("build_env");
        let actions = optional("stats/actions?id=latest");
        let mut summary = Summary::from_response(response, now_ms());
        summary.build_commit = build
            .as_ref()
            .and_then(|v| v.pointer("/git/commit_hash"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if summary.build_commit.is_none() {
            warnings.push("remote build commit unavailable".into());
        }
        summary.verification = actions.as_ref().and_then(verification);
        if summary.verification.is_none() {
            warnings.push("recent verification statistics unavailable".into());
        }
        Ok(summary)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

impl Summary {
    fn from_response(mut response: Response, now_ms: u64) -> Self {
        let mut source = None;
        if let Some(tip) = response.transition_frontier.best_tip.as_mut() {
            if tip.timestamp_ms.is_some() {
                source = Some("node_block");
            } else {
                // Compatibility with older /status: use only advertisements of
                // the exact local tip hash, never the newest arbitrary peer tip.
                let mut timestamps = response
                    .peers
                    .iter()
                    .filter(|p| {
                        p.connection_status == "Connected" && p.best_tip.as_ref() == Some(&tip.hash)
                    })
                    .filter_map(|p| p.best_tip_timestamp.map(|ns| ns / 1_000_000));
                if let Some(first) = timestamps.next() {
                    if timestamps.all(|other| other == first) {
                        tip.timestamp_ms = Some(first);
                        source = Some("peer_matching_tip");
                    }
                }
            }
        }
        let size = response
            .resources_status
            .pointer("/transition_frontier/best_chain_size")
            .and_then(Value::as_u64);
        let (root_height, root_source) = match response.transition_frontier.root.as_ref() {
            Some(root) => (Some(root.height), Some("node_root")),
            None => {
                // The applied best_chain is contiguous and includes root + tip.
                let derived = response
                    .transition_frontier
                    .best_tip
                    .as_ref()
                    .and_then(|tip| {
                        size.filter(|n| *n > 0)
                            .and_then(|n| tip.height.checked_sub(n - 1))
                    });
                (derived, derived.map(|_| "best_chain_size"))
            }
        };
        let mut peers = PeerCounts {
            connected: 0,
            libp2p: 0,
            webrtc: 0,
        };
        for peer in &response.peers {
            if peer.connection_status == "Connected" {
                peers.connected += 1;
                if peer.is_libp2p {
                    peers.libp2p += 1;
                } else {
                    peers.webrtc += 1;
                }
            }
        }
        let age = response
            .transition_frontier
            .best_tip
            .as_ref()
            .and_then(|t| t.timestamp_ms)
            .and_then(|timestamp| now_ms.checked_sub(timestamp))
            .map(|ms| ms / 1000);
        Self {
            // Compatibility mapping from OCaml Mesa chain-id vectors in
            // crates/core/src/chain_id.rs, not this CLI's --network setting.
            network: response
                .network
                .or_else(|| match response.chain_id.as_deref() {
                    Some("0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886") => {
                        Some("mainnet".into())
                    }
                    Some("ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8") => {
                        Some("devnet".into())
                    }
                    _ => None,
                }),
            chain_id: response.chain_id,
            sync_status: response.transition_frontier.sync.status,
            sync_phase: response.transition_frontier.sync.phase,
            best_tip: response.transition_frontier.best_tip,
            tip_age_secs: age,
            tip_timestamp_source: source,
            root_height,
            root_height_source: root_source,
            peers,
            build_commit: None,
            uptime_secs: response.uptime_secs,
            p2p_memory_bytes: response
                .resources_status
                .get("p2p_malloc_size")
                .and_then(Value::as_u64),
            verification: None,
        }
    }

    fn reasons(&self, max_age: u64, min_peers: usize) -> Vec<String> {
        let mut reasons = vec![];
        if self.sync_status != "Synced" {
            reasons.push(format!(
                "sync status {} ({})",
                one_line(&self.sync_status),
                one_line(&self.sync_phase)
            ));
        }
        match (&self.best_tip, self.tip_age_secs) {
            (None, _) => reasons.push("best tip unavailable".into()),
            (_, None) => reasons
                .push("best-tip timestamp unavailable or in the future (check clocks)".into()),
            (_, Some(age)) if age > max_age => {
                reasons.push(format!("tip age {age}s exceeds {max_age}s"))
            }
            _ => (),
        }
        if self.peers.connected < min_peers {
            reasons.push(format!(
                "connected peers {} below {min_peers}",
                self.peers.connected
            ));
        }
        reasons
    }
}

fn verification(value: &Value) -> Option<Verification> {
    if value.get("kind")?.as_str()? != "ForBlock" {
        return None;
    }
    let stats = value.get("stats")?.as_object()?;
    let count = |name: &str| -> Option<u64> {
        let buckets = stats.get(name)?.as_object()?;
        if buckets.is_empty() {
            return None;
        }
        buckets.values().try_fold(0_u64, |sum, bucket| {
            sum.checked_add(bucket.get("total_calls")?.as_u64()?)
        })
    };
    Some(Verification {
        since_unix_ns: value.get("time")?.as_u64()?,
        block_height: value.get("block_level")?.as_u64()?,
        block_hash: value.get("block_hash")?.as_str()?.into(),
        block_success: count("SnarkBlockVerifySuccess"),
        block_failures: count("SnarkBlockVerifyError"),
        work_success: count("SnarkWorkVerifySuccess"),
        work_failures: count("SnarkWorkVerifyError"),
    })
}

fn shown(value: Option<impl std::fmt::Display>) -> String {
    value
        .map(|v| one_line(&v.to_string()))
        .unwrap_or_else(|| "unavailable".into())
}

impl Report {
    fn health_line(&self) -> String {
        if self.healthy {
            "OK: synced, tip fresh, enough peers".into()
        } else {
            format!("UNHEALTHY: {}", self.reasons.join("; "))
        }
    }

    fn human(&self) -> String {
        let Some(s) = &self.status else {
            return format!("{}\n", self.health_line());
        };
        let mut output = format!(
            "Network: {}\nChain ID: {}\nSync: {} ({})\nBest tip: {} (age {} s; timestamp source {})\nRoot height: {} ({})\nPeers: {} connected ({} libp2p / {} WebRTC)\nBuild commit: {}\nUptime: {} s\nP2P memory: {} bytes (allocation estimate; process RSS unavailable)\n",
            shown(s.network.as_deref()), shown(s.chain_id.as_deref()), one_line(&s.sync_status), one_line(&s.sync_phase),
            shown(s.best_tip.as_ref().map(|t| t.height)), shown(s.tip_age_secs), shown(s.tip_timestamp_source),
            shown(s.root_height), shown(s.root_height_source), s.peers.connected, s.peers.libp2p, s.peers.webrtc,
            shown(s.build_commit.as_deref()), shown(s.uptime_secs), shown(s.p2p_memory_bytes),
        );
        if let Some(v) = &s.verification {
            output += &format!("Verification actions since tip change at height {} (Unix ns {}): blocks {} success / {} failed; work {} success / {} failed\n",
                v.block_height, v.since_unix_ns, shown(v.block_success), shown(v.block_failures), shown(v.work_success), shown(v.work_failures));
        } else {
            output += "Recent verification actions: unavailable\n";
        }
        output += &format!("{}\n", self.health_line());
        for warning in &self.warnings {
            output += &format!("Warning: {warning}\n");
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const STATUS: &str = include_str!("../../../tests/fixtures/status/relay2-status.json");
    const ACTIONS: &str = include_str!("../../../tests/fixtures/status/relay2-actions.json");
    const TIP_MS: u64 = 1_792_556_460_000;

    fn response() -> Response {
        serde_json::from_str(STATUS).unwrap()
    }

    #[test]
    fn recorded_relay2_response_and_threshold_boundaries() {
        let s = Summary::from_response(response(), TIP_MS + 1800_000);
        assert_eq!(s.network.as_deref(), Some("mainnet"));
        assert_eq!(s.best_tip.as_ref().unwrap().height, 564635);
        assert_eq!(s.root_height, Some(564345));
        assert_eq!(s.tip_age_secs, Some(1800));
        assert_eq!(s.tip_timestamp_source, Some("peer_matching_tip"));
        assert_eq!(
            (s.peers.connected, s.peers.libp2p, s.peers.webrtc),
            (6, 6, 0)
        );
        assert_eq!(s.p2p_memory_bytes, Some(1743469));
        assert_eq!(s.uptime_secs, None);
        assert!(s.reasons(1800, 6).is_empty());
        assert_eq!(s.reasons(1799, 7).len(), 2);
    }

    #[test]
    fn missing_tip_and_unsynced_are_unhealthy() {
        let mut r = response();
        r.transition_frontier.best_tip = None;
        r.transition_frontier.sync.status = "Bootstrap".into();
        let s = Summary::from_response(r, TIP_MS);
        assert_eq!(s.tip_age_secs, None);
        assert_eq!(s.root_height, None);
        assert_eq!(s.reasons(u64::MAX, 0).len(), 2);
    }

    #[test]
    fn only_matching_connected_peers_supply_legacy_timestamp() {
        let mut r = response();
        for p in &mut r.peers {
            p.best_tip = None;
        }
        let s = Summary::from_response(r, TIP_MS);
        assert_eq!(s.tip_age_secs, None);
        assert!(!s.reasons(u64::MAX, 0).is_empty());
        let mut r = response();
        let peer = r
            .peers
            .iter_mut()
            .find(|p| p.best_tip_timestamp == Some(TIP_MS * 1_000_000))
            .unwrap();
        peer.best_tip_timestamp = Some((TIP_MS + 1000) * 1_000_000);
        assert_eq!(Summary::from_response(r, TIP_MS).tip_age_secs, None);
    }

    #[test]
    fn future_timestamps_never_pass_as_zero_age() {
        let s = Summary::from_response(response(), TIP_MS - 1);
        assert_eq!(s.tip_age_secs, None);
        assert!(s.reasons(u64::MAX, 0)[0].contains("future"));
    }

    #[test]
    fn direct_fields_override_legacy_derivations_and_transport_is_counted() {
        let mut r = response();
        r.network = Some("custom".into());
        r.uptime_secs = Some(60);
        r.transition_frontier
            .best_tip
            .as_mut()
            .unwrap()
            .timestamp_ms = Some(TIP_MS);
        r.transition_frontier.root = Some(Tip {
            height: 564345,
            hash: "root".into(),
            timestamp_ms: None,
        });
        r.peers
            .iter_mut()
            .find(|p| p.connection_status == "Connected")
            .unwrap()
            .is_libp2p = false;
        let s = Summary::from_response(r, TIP_MS);
        assert_eq!(s.network.as_deref(), Some("custom"));
        assert_eq!(s.uptime_secs, Some(60));
        assert_eq!(s.tip_timestamp_source, Some("node_block"));
        assert_eq!(s.root_height_source, Some("node_root"));
        assert_eq!(
            (s.peers.connected, s.peers.libp2p, s.peers.webrtc),
            (6, 5, 1)
        );
    }

    #[test]
    fn unknown_chain_does_not_inherit_client_network() {
        let mut r = response();
        r.chain_id = Some("custom-chain".into());
        assert_eq!(Summary::from_response(r, TIP_MS).network, None);
    }

    #[test]
    fn recorded_verification_counts_and_missing_metrics() {
        let mut v: Value = serde_json::from_str(ACTIONS).unwrap();
        let stats = verification(&v).unwrap();
        assert_eq!(stats.block_height, 564635);
        assert_eq!(stats.since_unix_ns, 1792556467223097445);
        assert_eq!(stats.work_success, Some(0));
        assert_eq!(stats.block_failures, Some(0));
        v["stats"]
            .as_object_mut()
            .unwrap()
            .remove("SnarkWorkVerifyError");
        assert_eq!(verification(&v).unwrap().work_failures, None);
        v["stats"]["SnarkWorkVerifySuccess"]["under_1_us"]["total_calls"] = Value::Null;
        assert_eq!(verification(&v).unwrap().work_success, None);
        assert!(verification(&Value::Null).is_none());
    }

    #[test]
    fn malformed_required_status_is_rejected() {
        assert!(serde_json::from_str::<Response>("null").is_err());
        assert!(serde_json::from_str::<Response>(r#"{"peers":[]}"#).is_err());
        let mut v: Value = serde_json::from_str(STATUS).unwrap();
        v["peers"][0]["is_libp2p"] = Value::Null;
        assert!(serde_json::from_value::<Response>(v).is_err());
    }
}
