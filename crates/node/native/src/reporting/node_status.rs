//! The unsigned simplified node status report, OCaml's default.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L92-L143>

use super::json::Json;

/// `Node_status_service.Simplified.t`: seven fields, in this order, and no
/// `version`. The full version 2 report needs libp2p bandwidth and CPU,
/// per-RPC and gossip counters and received-block queues that this node
/// doesn't measure, so it isn't offered.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L92-L103>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimplifiedStatus {
    pub max_observed_block_height: u32,
    pub commit_hash: String,
    pub chain_id: String,
    pub peer_id: String,
    pub peer_count: usize,
    pub timestamp: String,
    pub block_producer_public_key: Option<String>,
}

impl SimplifiedStatus {
    /// `{"data":{...}}`, compact.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L105-L116>
    pub fn body(&self) -> String {
        Json::assoc([(
            "data",
            Json::assoc([
                (
                    "max_observed_block_height",
                    Json::Int(self.max_observed_block_height.into()),
                ),
                ("commit_hash", Json::str(&self.commit_hash)),
                ("chain_id", Json::str(&self.chain_id)),
                ("peer_id", Json::str(&self.peer_id)),
                ("peer_count", Json::Int(self.peer_count as i64)),
                ("timestamp", Json::str(&self.timestamp)),
                (
                    "block_producer_public_key",
                    Json::opt(self.block_producer_public_key.as_ref(), Json::str),
                ),
            ]),
        )])
        .to_compact_string()
    }
}
