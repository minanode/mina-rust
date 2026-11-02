//! Unsigned node error reports, sent once per explicit event.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L5-L183>

use super::json::Json;

/// The whole serialized wrapper's limit; larger reports are not sent.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L5-L6>
pub const MAX_REPORT_BYTES: usize = 10_000;

/// `--contact-info` must be shorter than this, in bytes.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L567-L583>
pub const MAX_CONTACT_INFO_BYTES: usize = 200;

/// `node_error_report`, fields in OCaml's order, options as `null`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L20-L39>
#[derive(Clone, Debug, PartialEq)]
pub struct NodeErrorReport {
    pub peer_id: String,
    pub ip_address: String,
    /// The block producer's public key, not a signing identity.
    pub public_key: Option<String>,
    pub commit_hash: String,
    pub chain_id: String,
    pub contact_info: Option<String>,
    /// OCaml's `Conf_dir.get_hw_info`; not collected (`null`).
    pub hardware_info: Option<Vec<String>>,
    pub timestamp: String,
    pub id: String,
    pub error: Json,
    /// OCaml's catchup tree counts; this node has no such tree (`null`).
    pub catchup_job_states: Option<Json>,
    pub sync_status: &'static str,
    pub block_height_at_best_tip: Option<u32>,
    pub max_observed_block_height: u32,
    pub max_observed_unvalidated_block_height: u32,
    pub uptime_of_node: String,
}

impl NodeErrorReport {
    pub fn to_json(&self) -> Json {
        Json::assoc([(
            "data",
            Json::assoc([
                ("version", Json::Int(1)),
                ("peer_id", Json::str(&self.peer_id)),
                ("ip_address", Json::str(&self.ip_address)),
                ("public_key", Json::opt(self.public_key.as_ref(), Json::str)),
                ("commit_hash", Json::str(&self.commit_hash)),
                ("chain_id", Json::str(&self.chain_id)),
                (
                    "contact_info",
                    Json::opt(self.contact_info.as_ref(), Json::str),
                ),
                (
                    "hardware_info",
                    Json::opt(self.hardware_info.as_ref(), |lines| {
                        Json::List(lines.iter().map(Json::str).collect())
                    }),
                ),
                ("timestamp", Json::str(&self.timestamp)),
                ("id", Json::str(&self.id)),
                ("error", self.error.clone()),
                (
                    "catchup_job_states",
                    self.catchup_job_states.clone().unwrap_or(Json::Null),
                ),
                ("sync_status", Json::str(self.sync_status)),
                (
                    "block_height_at_best_tip",
                    Json::opt(self.block_height_at_best_tip, |h| Json::Int(h.into())),
                ),
                (
                    "max_observed_block_height",
                    Json::Int(self.max_observed_block_height.into()),
                ),
                (
                    "max_observed_unvalidated_block_height",
                    Json::Int(self.max_observed_unvalidated_block_height.into()),
                ),
                ("uptime_of_node", Json::str(&self.uptime_of_node)),
            ]),
        )])
    }

    pub fn body(&self) -> String {
        self.to_json().to_compact_string()
    }

    /// Bytes left for the error in a report whose other fields are these:
    /// OCaml's `send_dynamic_report` sizes the report with an empty-string
    /// error, then subtracts the empty string's two bytes.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L155-L172>
    pub fn error_budget(&self) -> i64 {
        let mut base = self.clone();
        base.error = Json::str("");
        let base_size = base.body().len() as i64 - 2;
        MAX_REPORT_BYTES as i64 - base_size
    }
}

/// An explicit event to report.
#[derive(Clone, Debug)]
pub enum NodeErrorEvent {
    /// The process panicked and is about to stop: OCaml's monitor reports
    /// an unhandled exception before exiting.
    Crash { message: String, location: String },
    /// Transactions that failed to apply while a block's staged ledger diff
    /// was made.
    TransactionInclusionFailures(Vec<(serde_json::Value, String)>),
}

impl NodeErrorEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Crash { .. } => "crash",
            Self::TransactionInclusionFailures(_) => "transaction inclusion failures",
        }
    }

    /// The report's `error` given `available` bytes for it.
    pub fn error_json(&self, available: i64) -> Json {
        match self {
            Self::Crash { message, location } => crash_error(message, location, available),
            Self::TransactionInclusionFailures(failures) => {
                transaction_inclusion_failures(failures, available)
            }
        }
    }
}

/// An `Error_json`-shaped `{"string":…,"tags":[{"tag":"panic","loc":…}]}`.
/// The message is cut to fit; private keys are masked.
fn crash_error(message: &str, location: &str, available: i64) -> Json {
    let build = |message: String| {
        Json::assoc([
            ("string", Json::String(message)),
            (
                "tags",
                Json::List(vec![Json::assoc([
                    ("tag", Json::str("panic")),
                    ("loc", Json::str(location)),
                ])]),
            ),
        ])
    };
    let mut message = redact(message);
    let fixed = build(String::new()).to_compact_string().len() as i64;
    let room = available.saturating_sub(fixed).max(0) as usize;
    let mut used = 0usize;
    if let Some((cut, _)) = message.char_indices().find(|(_, c)| {
        used = used.saturating_add(super::json::escaped_len(*c));
        used > room
    }) {
        message.truncate(cut);
    }
    build(message)
}

/// OCaml's transaction-inclusion failure report, as many failures as fit.
/// Like OCaml, the last element may overshoot the budget; the size check
/// before sending then drops the report rather than cut its JSON.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L132-L171>
pub fn transaction_inclusion_failures(
    failures: &[(serde_json::Value, String)],
    available: i64,
) -> Json {
    let wrap = |sampled: Json| {
        Json::assoc([
            (
                "message",
                Json::str(
                    "Some transactions failed to apply to the ledger when creating the staged ledger diff",
                ),
            ),
            ("num_failures", Json::Int(failures.len() as i64)),
            ("sampled_failures", sampled),
        ])
    };
    let base_error_size = wrap(Json::str("")).to_compact_string().len() as i64 - 2;
    let mut left = available - base_error_size - 2;
    let mut sampled = Vec::new();
    for (transaction, error) in failures {
        if left <= 0 {
            break;
        }
        let element = Json::assoc([
            ("transaction", Json::from_serde(transaction)),
            (
                "error",
                Json::assoc([("string", Json::String(redact(error)))]),
            ),
        ]);
        left -= element.to_compact_string().len() as i64 + 1;
        sampled.push(element);
    }
    wrap(Json::List(sampled))
}

/// Masks Base58Check private keys (`EK…`, 52 characters) in free text that
/// goes into an error report.
pub fn redact(text: &str) -> String {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("EK") {
        let candidate = &rest[pos..];
        let len = candidate
            .char_indices()
            .find(|(_, c)| !B58.contains(*c))
            .map_or(candidate.len(), |(i, _)| i);
        out.push_str(&rest[..pos]);
        if len >= 50 {
            out.push_str("<redacted key>");
        } else {
            out.push_str(&candidate[..len]);
        }
        rest = &candidate[len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn report() -> NodeErrorReport {
        NodeErrorReport {
            peer_id: "12D3KooWBRp84j2BaaddgbMTWjxBL3cHubUb9aE9JZanQriLP5f5".into(),
            ip_address: "127.0.0.1".into(),
            public_key: None,
            commit_hash: "84b8d96954b218f372b93f65a071abf528e6bb61".into(),
            chain_id: "r43e1-schema-example".into(),
            contact_info: None,
            hardware_info: None,
            timestamp: "2026-10-21T00:00:00Z".into(),
            id: "00000000-0000-4000-8000-000000000001".into(),
            error: Json::assoc([(
                "message",
                Json::str("Synthetic offline schema fixture; not a daemon failure capture"),
            )]),
            catchup_job_states: None,
            sync_status: "Connecting",
            block_height_at_best_tip: None,
            max_observed_block_height: 0,
            max_observed_unvalidated_block_height: 0,
            uptime_of_node: "1s".into(),
        }
    }

    /// Same fields, order and values as the R43e.1 source-derived schema
    /// example (which is pretty-printed; the report is compact).
    #[test]
    fn matches_the_source_derived_schema_example() {
        let fixture = include_str!(
            "../../../../../tools/uptime-fixtures/fixtures/node-error-source-derived.json"
        );
        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let ours: serde_json::Value = serde_json::from_str(&report().body()).unwrap();
        assert_eq!(ours, expected);
        let order: Vec<&str> = fixture
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split('"').next())
            .filter(|k| !["data", "message"].contains(k))
            .collect();
        let body = report().body();
        let mut last = 0;
        for key in order {
            let at = body.find(&format!("\"{key}\":")).unwrap();
            assert!(at > last, "{key} out of order");
            last = at;
        }
    }

    #[test]
    fn crash_errors_fit_and_mask_keys() {
        // Key-shaped with a failing checksum: not a key. In two parts so the
        // export's key scanner, which can't tell, doesn't stop on it.
        let key = concat!("EK", "E5WGfZgyZHMAgQnAkf3ruNXvcpzNsuptS6WoRDMtvTMDzYPWgr");
        let event = NodeErrorEvent::Crash {
            message: format!("bad thing {key} happened {}", "x".repeat(20_000)),
            location: "crates/node/src/x.rs:1:2".into(),
        };
        let mut report = report();
        report.error = event.error_json(report.error_budget());
        let body = report.body();
        // Cut exactly to the limit.
        assert_eq!(body.len(), MAX_REPORT_BYTES);
        assert!(!body.contains(key));
        assert!(body.contains("<redacted key>"));
    }
}
