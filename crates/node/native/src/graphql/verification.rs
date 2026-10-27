//! Local verification telemetry; intentionally separate from Mina consensus data.
use juniper::GraphQLObject;
use mina_node::verification::{CheckReceipt, VerificationReceipt, VerificationSnapshot};

#[derive(GraphQLObject)]
pub struct LocalCheck {
    status: String,
    checked_at: Option<String>,
    elapsed_ms: Option<f64>,
}
impl From<CheckReceipt> for LocalCheck {
    fn from(v: CheckReceipt) -> Self {
        Self {
            status: match v.status {
                mina_node::verification::CheckStatus::NotRun => "notRun",
                mina_node::verification::CheckStatus::Pending => "pending",
                mina_node::verification::CheckStatus::Passed => "passed",
                mina_node::verification::CheckStatus::Failed => "failed",
                mina_node::verification::CheckStatus::Skipped => "skipped",
            }
            .to_owned(),
            checked_at: v.checked_at.map(|v| v.to_string()),
            elapsed_ms: v.elapsed_ms,
        }
    }
}
#[derive(GraphQLObject)]
pub struct LocalVerificationReceipt {
    state_hash: String,
    height: String,
    network: String,
    arrival: String,
    received_at: String,
    applied: bool,
    block_proof: LocalCheck,
    delta_chain: LocalCheck,
    body_reference: LocalCheck,
    verifier_key: Option<String>,
}
impl From<VerificationReceipt> for LocalVerificationReceipt {
    fn from(v: VerificationReceipt) -> Self {
        Self {
            state_hash: v.block.state_hash.to_string(),
            height: v.block.height.to_string(),
            network: v.network,
            arrival: arrival(v.arrival).to_owned(),
            received_at: v.received_at.to_string(),
            applied: v.applied,
            block_proof: v.block_proof.into(),
            delta_chain: v.delta_chain.into(),
            body_reference: v.body_reference.into(),
            verifier_key: v.verifier_key,
        }
    }
}
/// The most state hashes one `verificationReceipts` request looks up: a page
/// of the dashboard's recent blocks (`bestChain(maxLength: 20)`).
pub const MAX_PUBLIC_RECEIPT_HASHES: usize = 20;

/// One check of a [`PublicVerificationReceipt`]. A fixed public projection
/// of [`CheckReceipt`]: fields added there do not appear here.
#[derive(GraphQLObject)]
pub struct PublicVerificationCheck {
    /// `notRun`, `pending`, `passed`, `failed` or `skipped`.
    status: String,
    elapsed_ms: Option<f64>,
}
impl From<&CheckReceipt> for PublicVerificationCheck {
    fn from(v: &CheckReceipt) -> Self {
        let LocalCheck {
            status, elapsed_ms, ..
        } = v.clone().into();
        // Header checks can finish exactly at received_at. Publish durations
        // only: absolute check times disclose this node's ingress/scheduling.
        Self { status, elapsed_ms }
    }
}

/// What this node checked about one block, for public clients: the fields
/// are named one by one, so a field added to the private receipt
/// ([`VerificationReceipt`], `/stats/verification`) is not published.
#[derive(GraphQLObject)]
pub struct PublicVerificationReceipt {
    state_hash: String,
    height: String,
    network: String,
    /// How the block reached this node: `gossip`, `catchup`, `bootstrap`,
    /// `bestTip` or `local`.
    arrival: String,
    /// The block was applied: its staged ledger diff applied and gave the
    /// staged ledger hash in its header.
    applied: bool,
    block_proof: PublicVerificationCheck,
    delta_chain: PublicVerificationCheck,
    body_reference: PublicVerificationCheck,
    /// SHA-256 of the blockchain verifier index the proof was checked with.
    verifier_key: Option<String>,
}
impl From<&VerificationReceipt> for PublicVerificationReceipt {
    fn from(v: &VerificationReceipt) -> Self {
        Self {
            state_hash: v.block.state_hash.to_string(),
            height: v.block.height.to_string(),
            network: v.network.clone(),
            arrival: arrival(v.arrival).to_owned(),
            applied: v.applied,
            block_proof: (&v.block_proof).into(),
            delta_chain: (&v.delta_chain).into(),
            body_reference: (&v.body_reference).into(),
            verifier_key: v.verifier_key.clone(),
        }
    }
}

/// The receipts of up to [`MAX_PUBLIC_RECEIPT_HASHES`] blocks, by state
/// hash: one lookup on the node (O(log n) per hash in its 1,024 retained
/// receipts), answered in the order given, each block once. A block
/// without a retained receipt is left out.
pub async fn public_receipts(
    context: &super::Context,
    state_hashes: Vec<String>,
) -> juniper::FieldResult<Vec<PublicVerificationReceipt>> {
    use crate::input_limits::{bounded, STATE_HASH};
    use mina_node::{rpc::RpcRequest, verification::VerificationQuery};
    if state_hashes.len() > MAX_PUBLIC_RECEIPT_HASHES {
        return Err(format!(
            "verificationReceipts takes at most {MAX_PUBLIC_RECEIPT_HASHES} state hashes, got {}",
            state_hashes.len()
        )
        .into());
    }
    let hashes = state_hashes
        .iter()
        .map(|hash| {
            bounded(hash, STATE_HASH)
                .ok()
                .and_then(|hash| hash.parse().ok())
                .ok_or_else(|| juniper::FieldError::from(format!("Invalid state hash {hash:.52}")))
        })
        .collect::<juniper::FieldResult<Vec<_>>>()?;
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let snapshot = context
        .rpc_sender
        .oneshot_request::<mina_node::rpc::RpcVerificationStatsGetResponse>(
            RpcRequest::VerificationStatsGet(VerificationQuery::by_hash(hashes)),
        )
        .await
        .ok_or("Verification telemetry unavailable")?;
    Ok(snapshot.receipts.iter().map(Into::into).collect())
}

fn arrival(v: mina_node::verification::Arrival) -> &'static str {
    match v {
        mina_node::verification::Arrival::Gossip => "gossip",
        mina_node::verification::Arrival::Catchup => "catchup",
        mina_node::verification::Arrival::Bootstrap => "bootstrap",
        mina_node::verification::Arrival::BestTip => "bestTip",
        mina_node::verification::Arrival::Local => "local",
    }
}

pub struct LocalVerificationStats(VerificationSnapshot);

#[juniper::graphql_object(context = super::Context)]
impl LocalVerificationStats {
    fn capacity(&self) -> i32 {
        self.0.capacity as i32
    }
    fn verified_since_start(&self) -> String {
        self.0.verified_since_start.to_string()
    }
    fn failures_since_start(&self) -> String {
        self.0.failures_since_start.to_string()
    }
    fn recent_failure_count(&self) -> i32 {
        self.0.recent_failure_count as i32
    }
    fn median_proof_time_ms(&self) -> Option<f64> {
        self.0.median_proof_time_ms
    }
    fn last_verified(&self) -> Option<LocalVerificationReceipt> {
        self.0.last_verified.clone().map(Into::into)
    }

    /// Newest first by first observation; defaults to 64, maximum 128 receipts.
    async fn receipts(
        &self,
        context: &super::Context,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> juniper::FieldResult<Vec<LocalVerificationReceipt>> {
        use mina_node::{
            rpc::RpcRequest,
            verification::{VerificationQuery, MAX_RECEIPT_PAGE_SIZE},
        };
        if limit.is_some_and(|v| v < 0 || v as usize > MAX_RECEIPT_PAGE_SIZE)
            || offset.is_some_and(|v| v < 0)
        {
            return Err("Receipt limit must be 0..128 and offset must be non-negative".into());
        }
        let snapshot = context
            .rpc_sender
            .oneshot_request::<mina_node::rpc::RpcVerificationStatsGetResponse>(
                RpcRequest::VerificationStatsGet(VerificationQuery::page(
                    limit.map(|v| v as usize),
                    offset.unwrap_or(0) as usize,
                )),
            )
            .await
            .ok_or("Verification telemetry unavailable")?;
        Ok(snapshot.receipts.into_iter().map(Into::into).collect())
    }
}
impl From<VerificationSnapshot> for LocalVerificationStats {
    fn from(v: VerificationSnapshot) -> Self {
        Self(v)
    }
}

#[cfg(test)]
mod tests {
    use crate::graphql::{Context, Mutation, Query};
    use juniper::{http::GraphQLRequest, EmptySubscription, RootNode};
    use mina_node::{core::channels::mpsc, rpc::RpcRequest, verification::VerificationState};
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};

    #[tokio::test]
    async fn summary_query_never_requests_receipts_and_pages_are_lazy() {
        for (query, expected, succeeds) in [
            ("{ verification { verifiedSinceStart failuresSinceStart recentFailureCount medianProofTimeMs } }", vec![(Some(0), 0)], true),
            ("{ verification { receipts(limit: 2, offset: 3) { height } } }", vec![(Some(0), 0), (Some(2), 3)], true),
            ("{ verification { receipts { height } } }", vec![(Some(0), 0), (None, 0)], true),
            ("{ verification { receipts(limit: 128) { height } } }", vec![(Some(0), 0), (Some(128), 0)], true),
            ("{ verification { receipts(limit: 129) { height } } }", vec![(Some(0), 0)], false),
            ("{ verification { receipts(limit: -1) { height } } }", vec![(Some(0), 0)], false),
            ("{ verification { receipts(offset: -1) { height } } }", vec![(Some(0), 0)], false),
        ] {
            let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
            let server = tokio::spawn(async move {
                let mut seen = vec![];
                while let Some(NodeRpcRequest { req: request, responder }) = rx.recv().await {
                    let RpcRequest::VerificationStatsGet(query) = request else { panic!("unexpected RPC"); };
                    seen.push((query.limit, query.offset));
                    let response = VerificationState::default().snapshot(query);
                    let responder = responder.downcast::<mina_node::core::channels::oneshot::Sender<mina_node::rpc::RpcVerificationStatsGetResponse>>().unwrap();
                    let _ = responder.send(response);
                }
                seen
            });
            let context = Context::new(RpcSender::new(tx));
            let schema = RootNode::new(Query, Mutation, EmptySubscription::new());
            let response = GraphQLRequest::new(query.into(), None, None).execute(&schema, &context).await;
            let response = serde_json::to_value(&response).unwrap();
            assert_eq!(response.get("errors").is_none(), succeeds, "{response}");
            if !succeeds { assert!(response["errors"][0]["message"].as_str().unwrap().contains("Receipt limit")); }
            drop(context);
            assert_eq!(server.await.unwrap(), expected);
        }
    }
}
