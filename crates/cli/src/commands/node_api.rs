//! What the operator commands ask a running node: GraphQL at `<node>/graphql`
//! and the REST routes, over HTTP.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::Network;

/// Exit status when the node answered but refused the request (a rejected
/// transaction, a ledger it can't export).
pub const EXIT_NODE_REFUSED: i32 = 3;

/// Exit status when a recomputed ledger hash differs from the expected one.
pub const EXIT_HASH_MISMATCH: i32 = 4;

/// An error the node answered with, as opposed to one reaching it.
#[derive(Debug)]
pub struct NodeRefused(pub String);

impl std::fmt::Display for NodeRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NodeRefused {}

/// A recomputed ledger hash that differs from the expected one.
#[derive(Debug)]
pub struct HashMismatch(pub String);

impl std::fmt::Display for HashMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HashMismatch {}

/// Exits with the error and its status: [`EXIT_NODE_REFUSED`],
/// [`EXIT_HASH_MISMATCH`], or 1 for anything else.
pub fn exit_with(error: anyhow::Error) -> ! {
    let code = if error.downcast_ref::<NodeRefused>().is_some() {
        EXIT_NODE_REFUSED
    } else if error.downcast_ref::<HashMismatch>().is_some() {
        EXIT_HASH_MISMATCH
    } else {
        1
    };
    crate::exit_with_error_code(error, code)
}

pub fn client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .context("failed to create the HTTP client")
}

/// The node's base URL, without a trailing slash.
pub fn base_url(node: &str) -> &str {
    node.trim_end_matches('/')
}

/// The node's GraphQL URL, `<node>/graphql`. A value that already ends in
/// `/graphql`, as `--endpoint` and `internal graphql --node` took, is used as
/// given.
pub fn graphql_url(node: &str) -> String {
    let base = base_url(node);
    if base.ends_with("/graphql") {
        base.to_owned()
    } else {
        format!("{base}/graphql")
    }
}

/// Runs a GraphQL query with variables. GraphQL errors come back as
/// [`NodeRefused`].
pub fn graphql(node: &str, query: &str, variables: Value) -> Result<Value> {
    let url = graphql_url(node);
    let response = client(Duration::from_secs(120))?
        .post(&url)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .with_context(|| format!("failed to reach the node at {url}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .with_context(|| format!("the node at {url} answered HTTP {status} without JSON"))?;
    if let Some(errors) = body.get("errors").and_then(Value::as_array) {
        let messages: Vec<&str> = errors
            .iter()
            .map(|e| e.get("message").and_then(Value::as_str).unwrap_or("?"))
            .collect();
        return Err(NodeRefused(messages.join("; ")).into());
    }
    if !status.is_success() {
        bail!("the node at {url} answered HTTP {status}");
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

/// What the node says about itself.
pub struct NodeStatus {
    pub network_id: String,
    pub sync_status: String,
}

/// Checks the node is on `network`, and when `require_synced`, synced.
pub fn check_node(node: &str, network: &Network, require_synced: bool) -> Result<NodeStatus> {
    let data = graphql(node, "query { syncStatus networkID }", json!({}))?;
    let network_id = data["networkID"]
        .as_str()
        .context("the node did not say its networkID")?
        .to_owned();
    let sync_status = data["syncStatus"]
        .as_str()
        .context("the node did not say its syncStatus")?
        .to_owned();
    let expected = match network {
        Network::Mainnet => "mina:mainnet",
        Network::Devnet => "mina:devnet",
    };
    if network_id != expected {
        bail!(
            "the node is on {network_id}, not {expected}: pass the node's network with \
             --network (or MINA_NETWORK)"
        );
    }
    if require_synced && sync_status != "SYNCED" {
        bail!("the node is not synced (syncStatus {sync_status}); try again when it is");
    }
    Ok(NodeStatus {
        network_id,
        sync_status,
    })
}
