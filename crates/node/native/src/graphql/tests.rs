//! Response-shape tests against OCaml's GraphQL.
//!
//! The queries are the operations clients send: those of
//! `o1-labs/mina-sdk-spec`, the Auro wallet, o1js and OCaml's Rosetta server
//! (`tests/files/graphql/operations/`). They run on this node's schema
//! against a fake node service that answers from precomputed mainnet blocks. The expected responses are those an OCaml 4.0.2 mainnet daemon
//! gave to the same operations for the same blocks
//! (`tests/files/graphql/ocaml/`).

use std::{any::Any, path::PathBuf, sync::Arc};

use juniper::{http::GraphQLRequest, EmptySubscription, RootNode};
use ledger::{scan_state::currency::Balance, Account, AccountId};
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash, BlockWithHash},
    consensus::{ConsensusConstants, ConsensusTime},
    constants::constraint_constants,
    network::NetworkConfig,
};
use mina_node::{
    core::channels::{mpsc, oneshot},
    rpc::{
        AccountQuery, DelegatorsQuery, GetBlockQuery, RpcBestChainResponse,
        RpcConsensusConstantsGetResponse, RpcConsensusSlot, RpcGenesisBlockResponse,
        RpcGetBlockResponse, RpcLedgerAccountDelegatorsGetResponse, RpcLedgerAccountsResponse,
        RpcMetricsGetResponse, RpcNextBlockProduction, RpcNodeStatus, RpcNodeStatusLedger,
        RpcNodeStatusNetworkInfo, RpcNodeStatusResources, RpcNodeStatusTransitionFrontier,
        RpcNodeStatusTransitionFrontierSync, RpcPeersGetResponse, RpcPooledUserCommandsResponse,
        RpcPooledZkappCommandsResponse, RpcRequest, RpcStatusGetResponse, RpcSyncStatsGetResponse,
        RpcVerificationStatsGetResponse,
    },
    verification::{BlockIdentity, CheckStatus, VerificationState, VerificationUpdate},
};
use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
use mina_p2p_messages::{
    list::List,
    v2::{
        MinaBlockBlockStableV2, MinaBlockHeaderStableV2, ProtocolVersionStableV2,
        StagedLedgerDiffBodyStableV1, StateHash,
    },
};
use serde_json::{json, Value};

use super::{Context, Mutation, Query};

type Schema = RootNode<Query, Mutation, EmptySubscription<Context>>;

#[test]
fn pc2_graphql_cached_authorization_matches_and_unavailable_is_an_error() {
    use mina_p2p_messages::{binprot::BinProtRead, proof_cache::ProofStore, v2};
    fn find(
        node: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    ) -> Option<v2::MinaBaseControlStableV2> {
        if matches!(
            node.account_update.authorization,
            v2::MinaBaseControlStableV2::Proof(_)
        ) {
            return Some(node.account_update.authorization.clone());
        }
        node.calls.iter().find_map(|call| find(&call.elt))
    }
    let bytes = include_bytes!("../../../../../tests/files/zkapps/with_proof_auth.bin");
    let command =
        v2::MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut bytes.as_slice()).unwrap();
    let auth = command
        .account_updates
        .iter()
        .find_map(|u| find(&u.elt))
        .unwrap();
    let expected = super::zkapp::GraphQLAuthorization::try_from(auth.clone()).unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../target/test-data/proof-cache/graphql-{}",
        std::process::id()
    ));
    let store = ProofStore::dir(&dir).unwrap();
    let v2::MinaBaseControlStableV2::Proof(proof) = &auth else {
        unreachable!()
    };
    proof.cache_in(&store).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..2000 {
        let got = super::zkapp::GraphQLAuthorization::try_from(auth.clone()).unwrap();
        assert_eq!(got.proof, expected.proof);
        assert_eq!(got.signature, expected.signature);
    }
    println!(
        "PC2 GraphQL cached proof authorization: {:?} mean of 2000",
        start.elapsed() / 2000
    );
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(file).unwrap();
    assert!(super::zkapp::GraphQLAuthorization::try_from(auth.clone()).is_err());
    drop((auth, command));
    assert_eq!(store.stats().files, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

fn files() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/files/graphql")
}

/// The blocks OCaml's responses are about: the three tip blocks of mainnet
/// when they were taken.
const BLOCKS: &[(u32, &str)] = &[
    (
        564493,
        "3NK5Kp6Qu2CrGJCt3cbRmM11Ehn11WhQSGUtfEmCNUXVx7rk33Xw",
    ),
    (
        564494,
        "3NL4BmPU4varBDBWXE8qjZLXKiZD9xqN12rBec2uuAwTwQ5JwQmb",
    ),
    (
        564495,
        "3NKoJTEU2aR6MNRrm2rp5P758LcLYdfY6ShXpNNPxEr19hGDod5Z",
    ),
];

fn init_network() {
    // Another test may have set it already.
    let _ = NetworkConfig::init("mainnet");
}

/// A precomputed block as the node keeps it. The proof and protocol
/// versions are not shown by GraphQL, so they are placeholders.
fn load_block(height: u32, hash: &str) -> AppliedBlock {
    load_block_from(&files().join("blocks"), height, hash)
}

fn load_block_from(dir: &std::path::Path, height: u32, hash: &str) -> AppliedBlock {
    let block = mina_block_replay::read_block(dir, height, hash).expect("precomputed block");
    let previous = block.protocol_state.previous_state_hash.clone();
    let block = MinaBlockBlockStableV2 {
        header: MinaBlockHeaderStableV2 {
            protocol_state: block.protocol_state,
            protocol_state_proof: mina_core::dummy::dummy_blockchain_proof(),
            delta_block_chain_proof: (previous, List::new()),
            current_protocol_version: ProtocolVersionStableV2 {
                transaction: 4u64.into(),
                network: 0u64.into(),
                patch: 0u64.into(),
            },
            proposed_protocol_version_opt: None,
        },
        body: StagedLedgerDiffBodyStableV1 {
            staged_ledger_diff: block.staged_ledger_diff,
        },
    };
    let block = BlockWithHash::try_new(Arc::new(block)).expect("block hash");
    assert_eq!(block.hash.to_string(), hash, "state hash of block {height}");
    AppliedBlock {
        block,
        just_emitted_a_proof: false,
    }
}

/// Answers the requests the GraphQL resolvers make, from a best chain and a
/// ledger. Requests it does not know get no answer, as when the node drops
/// them.
#[derive(Clone, Default)]
struct FakeNode {
    best_chain: Vec<AppliedBlock>,
    accounts: Vec<Account>,
    genesis: Option<ArcBlockWithHash>,
    receipts: VerificationState,
    /// The best tip's staking epoch ledger.
    staking_accounts: Vec<Account>,
    /// What the ledger service answers for `DelegatorsQuery::LastEpoch`.
    last_epoch_delegators: Option<Vec<Account>>,
    expected_last_epoch_limit: Option<Option<usize>>,
    status: Option<RpcNodeStatus>,
    metrics: Option<RpcMetricsGetResponse>,
    staking_lookups: Option<Arc<std::sync::atomic::AtomicUsize>>,
    /// The ledger light account reads answer from (LN2); `None`: the node
    /// is not synced.
    light_ledger: Option<ledger::Mask>,
    /// The block whose ledger `light_ledger` is.
    light_block: Option<mina_p2p_messages::v2::StateHash>,
    light_lookups: Arc<std::sync::atomic::AtomicUsize>,
}

fn respond<T: 'static + Send>(responder: Box<dyn Send + Any>, response: T) {
    let sender = responder
        .downcast::<oneshot::Sender<T>>()
        .expect("the resolver expects another response type");
    let _ = sender.send(response);
}

impl FakeNode {
    fn handle(&self, NodeRpcRequest { req, responder }: NodeRpcRequest) {
        match req {
            RpcRequest::BestChain(max_length) => {
                let skip = self.best_chain.len().saturating_sub(max_length as usize);
                let response: RpcBestChainResponse =
                    self.best_chain.iter().skip(skip).cloned().collect();
                respond(responder, response)
            }
            RpcRequest::GetBlock(query) => {
                let response: RpcGetBlockResponse = self
                    .best_chain
                    .iter()
                    .find(|block| match &query {
                        GetBlockQuery::Hash(hash) => &block.block.hash == hash,
                        GetBlockQuery::Height(height) => block.block.height() == *height,
                    })
                    .cloned();
                respond(responder, response)
            }
            RpcRequest::LedgerAccountsGet(AccountQuery::StakingEpochIds(ids)) => {
                if let Some(count) = &self.staking_lookups {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                let response: RpcLedgerAccountsResponse = self
                    .staking_accounts
                    .iter()
                    .filter(|account| ids.contains(&account.id()))
                    .cloned()
                    .collect();
                respond(responder, response)
            }
            RpcRequest::LedgerAccountDelegatorsGet(query, id) => {
                let response: RpcLedgerAccountDelegatorsGetResponse = match query {
                    DelegatorsQuery::LastEpoch { limit } => {
                        if let Some(expected) = self.expected_last_epoch_limit {
                            assert_eq!(limit, expected);
                        }
                        self.last_epoch_delegators.clone()
                    }
                    DelegatorsQuery::Ledger(_) => Some(
                        self.staking_accounts
                            .iter()
                            .filter(|account| account.delegate.as_ref() == Some(&id.public_key))
                            .cloned()
                            .collect(),
                    ),
                };
                respond(responder, response)
            }
            RpcRequest::LedgerAccountsGet(query) => {
                let response: RpcLedgerAccountsResponse = self
                    .accounts
                    .iter()
                    .filter(|account| match &query {
                        AccountQuery::All => true,
                        AccountQuery::SinglePublicKey(public_key) => {
                            mina_signer::CompressedPubKey::try_from(public_key.clone()).ok()
                                == Some(account.public_key.clone())
                        }
                        AccountQuery::MultipleIds(ids) => ids.contains(&account.id()),
                        AccountQuery::PubKeyWithTokenId(public_key, token_id) => {
                            mina_signer::CompressedPubKey::try_from(public_key.clone()).ok()
                                == Some(account.public_key.clone())
                                && ledger::TokenId::try_from(token_id).ok()
                                    == Some(account.token_id.clone())
                        }
                        AccountQuery::TokenOwner(token_id) => {
                            ledger::TokenId::try_from(token_id).ok()
                                == Some(account.id().derive_token_id())
                        }
                        AccountQuery::StakingEpochIds(_) => unreachable!("answered above"),
                    })
                    .cloned()
                    .collect();
                respond(responder, response)
            }
            RpcRequest::LightAccount(query) => {
                use ledger::BaseLedger;
                use mina_node::rpc::{
                    LightBlockRef, LightMerklePathElement, RpcLightAccount,
                    RpcLightAccountResponse, RpcLightError,
                };
                self.light_lookups
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let block = self.light_block.clone().unwrap_or_else(StateHash::zero);
                let response: RpcLightAccountResponse = match self.light_ledger.clone() {
                    None => Err(RpcLightError::NotSynced),
                    Some(_) if query.block.as_ref().is_some_and(|hash| *hash != block) => {
                        Err(RpcLightError::BlockNotInBestChain(query.block.unwrap()))
                    }
                    Some(mut mask) => Ok(query
                        .account_ids
                        .iter()
                        .map(|id| match mask.location_of_account(id) {
                            None => Err(RpcLightError::AccountNotFound(query.ledger)),
                            Some(address) => {
                                let account = mask.get(address.clone()).unwrap();
                                Ok(Box::new(RpcLightAccount {
                                    ledger: query.ledger,
                                    ledger_hash: mina_p2p_messages::v2::LedgerHash::from_fp(
                                        mask.merkle_root(),
                                    ),
                                    block: LightBlockRef {
                                        state_hash: block.clone(),
                                        height: 1,
                                        global_slot_since_genesis: 1,
                                    },
                                    index: address.to_index().as_u64(),
                                    leaf_hash: (&account.hash()).into(),
                                    account: (&*account).into(),
                                    merkle_path: mask
                                        .merkle_path(address)
                                        .iter()
                                        .map(LightMerklePathElement::from)
                                        .collect(),
                                }))
                            }
                        })
                        .collect()),
                };
                respond(responder, response)
            }
            RpcRequest::ConsensusConstantsGet => {
                let block = self.best_chain.last().expect("a best tip");
                let response: RpcConsensusConstantsGetResponse = ConsensusConstants::create(
                    constraint_constants(),
                    &block.block.header().protocol_state.body.constants,
                );
                respond(responder, response)
            }
            RpcRequest::ConsensusTimeGet(_) => respond(
                responder,
                Some(ConsensusTime {
                    start_time: redux::Timestamp::new(1_788_458_400_000_000_000),
                    end_time: redux::Timestamp::new(1_788_458_490_000_000_000),
                    epoch: 0,
                    global_slot: 0,
                    slot: 0,
                }),
            ),
            RpcRequest::GenesisBlockGet => {
                respond::<RpcGenesisBlockResponse>(responder, self.genesis.clone())
            }
            RpcRequest::StatusGet => {
                respond::<RpcStatusGetResponse>(responder, self.status.clone())
            }
            RpcRequest::MetricsGet => {
                if let Some(snapshot) = &self.metrics {
                    respond(responder, snapshot.clone());
                }
            }
            RpcRequest::SyncStatsGet(_) => respond::<RpcSyncStatsGetResponse>(responder, None),
            RpcRequest::PeersGet => respond::<RpcPeersGetResponse>(responder, vec![]),
            RpcRequest::PooledUserCommands(_) => {
                respond::<RpcPooledUserCommandsResponse>(responder, vec![])
            }
            RpcRequest::PooledZkappCommands(_) => {
                respond::<RpcPooledZkappCommandsResponse>(responder, vec![])
            }
            RpcRequest::VerificationStatsGet(query) => {
                respond::<RpcVerificationStatsGetResponse>(responder, self.receipts.snapshot(query))
            }
            _ => {}
        }
    }

    fn spawn(self) -> RpcSender {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                self.handle(request);
            }
        });
        RpcSender::new(tx)
    }
}

/// The named operations of a GraphQL document, as (kind, name, text).
fn operations(document: &str) -> Vec<(String, String, String)> {
    let mut ops: Vec<(String, String, String)> = Vec::new();
    for line in document.lines() {
        let kind = ["query ", "mutation "]
            .into_iter()
            .find(|kind| line.starts_with(kind));
        if let Some(kind) = kind {
            let name: String = line[kind.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            ops.push((kind.trim().to_owned(), name, String::new()));
        }
        if let Some((_, _, text)) = ops.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    ops
}

fn client_operations(file: &str) -> Vec<(String, String, String)> {
    let path = files().join("operations").join(file);
    operations(&std::fs::read_to_string(path).unwrap())
}

fn sdk_operations() -> Vec<(String, String, String)> {
    client_operations("sdk.graphql")
}

fn sdk_operation(name: &str) -> String {
    sdk_operations()
        .into_iter()
        .find(|(_, op, _)| op == name)
        .unwrap_or_else(|| panic!("no operation {name}"))
        .2
}

/// Executes one operation as the HTTP handler does, and returns the
/// response as JSON.
async fn execute(node: FakeNode, operation_name: &str, query: &str, variables: Value) -> Value {
    let request: GraphQLRequest = serde_json::from_value(json!({
        "query": query,
        "operationName": operation_name,
        "variables": variables,
    }))
    .unwrap();
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(node.spawn());
    let response = request.execute(&schema, &context).await;
    serde_json::to_value(&response).unwrap()
}

fn ocaml_response(name: &str) -> Value {
    let path = files().join("ocaml").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Asserts two JSON values are equal, naming the first path that differs.
fn assert_json_eq(ours: &Value, ocaml: &Value, path: &str) {
    match (ours, ocaml) {
        (Value::Object(a), Value::Object(b)) => {
            // Other workspace packages enable serde_json/preserve_order.
            // Object member order is immaterial; array order still matters.
            let mut keys_a: Vec<_> = a.keys().collect();
            let mut keys_b: Vec<_> = b.keys().collect();
            keys_a.sort_unstable();
            keys_b.sort_unstable();
            assert_eq!(keys_a, keys_b, "keys at {path}");
            for (key, value) in a {
                assert_json_eq(value, &b[key], &format!("{path}.{key}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "length at {path}");
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                assert_json_eq(x, y, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(ours, ocaml, "at {path}"),
    }
}

fn best_chain_node() -> FakeNode {
    init_network();
    let best_chain: Vec<AppliedBlock> = BLOCKS
        .iter()
        .map(|(h, hash)| load_block(*h, hash))
        .collect();
    // Receipts for the two newer blocks, as the node records them when it
    // receives, verifies and applies a block; none for the oldest.
    let mut receipts = VerificationState::default();
    for (i, applied) in best_chain.iter().enumerate().skip(1) {
        let block = BlockIdentity::new(&applied.block);
        let time = redux::Timestamp::new(1_700_000_000_000_000_000 + i as u64 * 1_000_000_000);
        receipts.record(
            &VerificationUpdate::Headers {
                block: block.clone(),
                arrival: mina_node::verification::Arrival::Gossip,
                delta: CheckStatus::Passed,
                body: CheckStatus::Passed,
            },
            time,
        );
        receipts.record(
            &VerificationUpdate::Proof {
                block,
                status: CheckStatus::Passed,
                elapsed_ms: Some(1500.5),
                verifier_key: Some("ab".repeat(32)),
            },
            time,
        );
        receipts.applied(&applied.block, time, true);
    }
    FakeNode {
        best_chain,
        receipts,
        metrics: Some(metrics_snapshot("Synced")),
        ..Default::default()
    }
}

/// The validation errors of each operation of a client's file, for those
/// that have any. Variables are not given, so required ones are reported
/// missing, which is not a schema difference and is left out.
async fn validation_errors(file: &str) -> Vec<(String, Vec<String>)> {
    let mut invalid = Vec::new();
    for (_, name, text) in client_operations(file) {
        let response = execute(best_chain_node(), &name, &text, json!({})).await;
        let errors = response["errors"].as_array().cloned().unwrap_or_default();
        // Validation errors have no path; resolver errors do.
        let messages: Vec<String> = errors
            .iter()
            .filter(|e| e.get("path").is_none())
            .filter_map(|e| e["message"].as_str().map(str::to_owned))
            .filter(|m| !m.contains("was not provided"))
            .collect();
        if !messages.is_empty() {
            invalid.push((name, messages));
        }
    }
    invalid
}

async fn invalid_operations(file: &str) -> Vec<String> {
    validation_errors(file)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The client operations our schema accepts, and those it does not yet:
/// the send paths (P6), and queries this round leaves for later. Each one
/// fixed moves out of these lists.
#[tokio::test]
async fn client_operations_validate() {
    assert_eq!(
        sdk_operations().len(),
        22,
        "mina-sdk-spec has 22 operations"
    );
    assert_eq!(
        invalid_operations("sdk.graphql").await,
        [
            "ForkConfig",
            "UnlockAccount",
            "SetSnarkWorker",
            "SetSnarkWorkFee",
        ]
    );
    assert_eq!(
        invalid_operations("o1js.graphql").await,
        Vec::<String>::new()
    );
    assert_eq!(invalid_operations("auro.graphql").await, AURO_NOT_YET);
    assert_eq!(invalid_operations("rosetta.graphql").await, ROSETTA_NOT_YET);
}

/// A metrics snapshot, as `MetricsGet` answers: `syncStatus` reads the
/// phase from it, `daemonStatus { metrics }` the rest.
fn metrics_snapshot(sync_phase: &str) -> RpcMetricsGetResponse {
    RpcMetricsGetResponse {
        chain_id: None,
        uptime_secs: 1,
        sync_phase: sync_phase.into(),
        ready: false,
        best_tip_height: None,
        best_tip_global_slot: None,
        highest_peer_tip_height: None,
        peers_connecting: 0,
        peers_connected: 0,
        peers_disconnecting: 0,
        peers_disconnected: 0,
        transaction_pool_size: 8,
        transaction_pool_diff_received: 11,
        transaction_pool_diff_broadcasted: 2,
        transactions_added_to_pool: 5,
        snark_pool_jobs: 14,
        snark_pool_completed: 6,
        snark_pool_diff_received: 13,
        snark_pool_diff_broadcasted: 4,
        block_production_delay_buckets: [1, 2, 0, 0, 0, 0, 1],
        block_production_delay_sum_ms: 300_000,
        snark_work_received_total: 0,
        verification: VerificationState::default().summary(),
        block_producer_slots_won: 0,
        block_producer_blocks_produced: 0,
        block_producer_blocks_failed: 0,
        snark_worker_proofs_made: 0,
        snark_worker_proofs_failed: 0,
        snark_worker_proofs_sold: 0,
        ledger_sync_fetched: None,
        ledger_sync_estimated: None,
    }
}

/// F-dash3: `syncStatus` is the phase `/status` reports, OCaml's names for
/// the phases before the node hears from the network included.
#[tokio::test]
async fn sync_status_is_the_reported_phase() {
    for (phase, expected) in [
        ("Connecting", "CONNECTING"),
        ("Listening", "LISTENING"),
        ("Bootstrap", "BOOTSTRAP"),
        ("Catchup", "CATCHUP"),
        ("Synced", "SYNCED"),
    ] {
        let mut node = FakeNode::default();
        node.metrics = Some(metrics_snapshot(phase));
        let response = execute(
            node,
            "S",
            "query S { syncStatus daemonStatus { syncStatus } }",
            json!({}),
        )
        .await;
        assert_eq!(response["data"]["syncStatus"], expected, "{phase}");
        assert_eq!(
            response["data"]["daemonStatus"]["syncStatus"], expected,
            "{phase}"
        );
    }
}

#[tokio::test]
async fn daemon_metrics_keep_ocaml_non_null_shape_and_snapshot_values() {
    let mut node = FakeNode::default();
    node.metrics = Some(metrics_snapshot("Synced"));
    let response = execute(
        node,
        "DaemonMetrics",
        &sdk_operation("DaemonMetrics"),
        json!({}),
    )
    .await;
    assert_eq!(
        response["data"]["daemonStatus"]["metrics"],
        json!({
            "blockProductionDelay": [1, 2, 0, 0, 0, 0, 1],
            "transactionPoolDiffReceived": 11,
            "transactionPoolDiffBroadcasted": 2,
            "transactionsAddedToPool": 5,
            "transactionPoolSize": 8,
            "snarkPoolDiffReceived": 13,
            "snarkPoolDiffBroadcasted": 4,
            "pendingSnarkWork": 8,
            "snarkPoolSize": 6,
        })
    );

    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let request: GraphQLRequest = serde_json::from_value(json!({
        "query": "{ __type(name: \"Metrics\") { fields { name type { kind } } } }"
    }))
    .unwrap();
    let context = Context::new(FakeNode::default().spawn());
    let introspection = serde_json::to_value(&request.execute(&schema, &context).await).unwrap();
    for field in [
        "blockProductionDelay",
        "transactionPoolDiffReceived",
        "transactionPoolDiffBroadcasted",
        "transactionsAddedToPool",
        "snarkPoolDiffReceived",
        "snarkPoolDiffBroadcasted",
    ] {
        let actual = introspection["data"]["__type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["name"] == field)
            .unwrap();
        assert_eq!(actual["type"]["kind"], "NON_NULL", "{field}");
    }
}

const AURO_NOT_YET: &[&str] = &[];
const ROSETTA_NOT_YET: &[&str] = &[];

/// Prints every validation error of the client operations, for the gap
/// list: `cargo test ... print_validation_errors -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn print_validation_errors() {
    for file in [
        "sdk.graphql",
        "o1js.graphql",
        "auro.graphql",
        "rosetta.graphql",
    ] {
        for (name, messages) in validation_errors(file).await {
            println!("{file} {name}: {}", messages.join("; "));
        }
    }
}

/// Writes this node's schema as introspection JSON to the path in
/// `GRAPHQL_INTROSPECTION_OUT`, to compare with OCaml's
/// `graphql_schema.json`.
#[tokio::test]
#[ignore]
async fn write_introspection() {
    let Ok(path) = std::env::var("GRAPHQL_INTROSPECTION_OUT") else {
        return;
    };
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(best_chain_node().spawn());
    let (value, errors) =
        juniper::introspect(&schema, &context, juniper::IntrospectionFormat::default()).unwrap();
    assert!(errors.is_empty());
    let json = serde_json::to_string_pretty(&json!({ "data": value })).unwrap();
    std::fs::write(path, json).unwrap();
}

#[tokio::test]
async fn best_chain_matches_ocaml() {
    let response = execute(
        best_chain_node(),
        "BestChain",
        &sdk_operation("BestChain"),
        json!({ "maxLength": 3 }),
    )
    .await;
    assert_eq!(response.get("errors"), None);
    assert_json_eq(&response, &ocaml_response("BestChain"), "");
}

#[tokio::test]
async fn block_by_height_matches_ocaml() {
    let response = execute(
        best_chain_node(),
        "Block",
        &sdk_operation("Block"),
        json!({ "stateHash": null, "height": 564495 }),
    )
    .await;
    assert_eq!(response.get("errors"), None);
    assert_json_eq(&response, &ocaml_response("Block"), "");
}

#[tokio::test]
async fn block_by_state_hash_matches_ocaml() {
    let response = execute(
        best_chain_node(),
        "Block",
        &sdk_operation("Block"),
        json!({ "stateHash": BLOCKS[2].1, "height": null }),
    )
    .await;
    assert_eq!(response.get("errors"), None);
    assert_json_eq(&response, &ocaml_response("Block"), "");
}

#[tokio::test]
async fn account_matches_ocaml() {
    let mut node = best_chain_node();
    let public_key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let account_id = AccountId::new_with_default_token(
        mina_signer::CompressedPubKey::from_address(public_key).unwrap(),
    );
    node.accounts.push(Account::create_with(
        account_id,
        Balance::from_u64(6_500_000_000),
    ));
    let response = execute(
        node,
        "Account",
        &sdk_operation("Account"),
        json!({ "publicKey": public_key, "token": null }),
    )
    .await;
    assert_eq!(response.get("errors"), None);
    assert_json_eq(&response, &ocaml_response("Account"), "");
}

/// OCaml answers null for an account that is not in the ledger.
#[tokio::test]
async fn missing_account_is_null() {
    let response = execute(
        best_chain_node(),
        "Account",
        &sdk_operation("Account"),
        json!({
            "publicKey": "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg",
            "token": null,
        }),
    )
    .await;
    assert_eq!(response, json!({ "data": { "account": null } }));
}

#[tokio::test]
async fn genesis_constants_match_ocaml() {
    let response = execute(
        best_chain_node(),
        "GenesisConstants",
        &sdk_operation("GenesisConstants"),
        json!({}),
    )
    .await;
    assert_json_eq(&response, &ocaml_response("GenesisConstants"), "");
}

/// The explorer's consensus-clock selection from D2.
#[tokio::test]
async fn explorer_consensus_clock_query() {
    let query = r#"query ExplorerConsensusClock {
      daemonStatus {
        consensusTimeNow { globalSlot startTime endTime }
        consensusTimeBestTip { globalSlot }
        globalSlotSinceGenesisBestTip
        consensusConfiguration {
          slotsPerEpoch
          slotDuration
          genesisStateTimestamp
        }
      }
    }"#;
    let response = execute(
        best_chain_node(),
        "ExplorerConsensusClock",
        query,
        json!({}),
    )
    .await;
    assert_eq!(response.get("errors"), None, "{response}");
    assert_eq!(
        response["data"]["daemonStatus"]["consensusConfiguration"]["genesisStateTimestamp"],
        "2026-09-03 18:00:00.000000Z"
    );
}

#[tokio::test]
async fn simple_queries_match_ocaml() {
    for name in ["NetworkId", "TrackedAccounts"] {
        let response = execute(best_chain_node(), name, &sdk_operation(name), json!({})).await;
        assert_json_eq(&response, &ocaml_response(name), name);
    }
}

/// What the block explorer asks of a block: every kind of transaction.
const EXPLORER_BLOCK: &str = r#"
query ExplorerBlock($stateHash: String) {
  block(stateHash: $stateHash) {
    stateHash
    commandTransactionCount
    protocolState { consensusState { blockHeight blockCreator coinbaseReceiever } }
    snarkJobs { prover fee workIds }
    transactions {
      coinbase
      coinbaseReceiverAccount { publicKey }
      feeTransfer { recipient fee type }
      userCommands {
        hash kind nonce amount fee memo failureReason
        source { publicKey }
        receiver { publicKey }
      }
      zkappCommands {
        hash
        failureReason { index failures }
        zkappCommand {
          feePayer { body { publicKey nonce fee } }
          accountUpdates { body { publicKey callDepth } }
        }
      }
    }
  }
}
"#;

/// Runs [`EXPLORER_BLOCK`] on one fixture block, and returns the block and
/// the precomputed block's JSON.
async fn explorer_block(height: u32, hash: &str) -> (Value, Value) {
    init_network();
    let node = FakeNode {
        best_chain: vec![load_block(height, hash)],
        ..Default::default()
    };
    let response = execute(
        node,
        "ExplorerBlock",
        EXPLORER_BLOCK,
        json!({ "stateHash": hash }),
    )
    .await;
    assert_eq!(response.get("errors"), None, "{response}");
    let path = files().join(format!("blocks/{height}-{hash}.json.zst"));
    let bytes = zstd::decode_all(std::fs::File::open(path).unwrap()).unwrap();
    // Sok digests are byte strings that need not be UTF-8: lossy is enough
    // to read the rest.
    let precomputed: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    let precomputed = precomputed.get("data").cloned().unwrap_or(precomputed);
    (response["data"]["block"].clone(), precomputed)
}

/// The commands of a precomputed block, in block order.
fn precomputed_commands(precomputed: &Value) -> Vec<Value> {
    precomputed["staged_ledger_diff"]["diff"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pre_diff| !pre_diff.is_null())
        .flat_map(|pre_diff| pre_diff["commands"].as_array().unwrap().clone())
        .collect()
}

/// A zkApp command with nested calls, a payment and a delegation: commands
/// come last first, a delegation has amount 0 and its new delegate as
/// receiver, and every account update shows, each parent before its calls.
#[tokio::test]
async fn explorer_block_with_zkapp_and_delegation() {
    let (block, precomputed) = explorer_block(
        548270,
        "3NK6i82CVVcP4KrnHmsc1rLktocbAEUSmnNDShUjKUvP3BoVDSE3",
    )
    .await;
    let commands = precomputed_commands(&precomputed);
    assert_eq!(block["commandTransactionCount"], json!(commands.len()));

    let user_commands = block["transactions"]["userCommands"].as_array().unwrap();
    let kinds: Vec<_> = user_commands.iter().map(|c| c["kind"].clone()).collect();
    assert_eq!(kinds, [json!("PAYMENT"), json!("STAKE_DELEGATION")]);
    let delegation = &user_commands[1];
    let precomputed_delegation = commands
        .iter()
        .find(|c| c["data"][1]["payload"]["body"][0] == "Stake_delegation")
        .unwrap();
    assert_eq!(delegation["amount"], "0");
    assert_eq!(
        delegation["receiver"]["publicKey"],
        precomputed_delegation["data"][1]["payload"]["body"][1][1]["new_delegate"]
    );
    assert_eq!(
        delegation["source"]["publicKey"],
        precomputed_delegation["data"][1]["payload"]["common"]["fee_payer_pk"]
    );

    fn walk(forest: &Value, depth: i64, out: &mut Vec<Value>) {
        for tree in forest.as_array().unwrap() {
            let update = &tree["elt"]["account_update"]["body"];
            out.push(json!({ "body": { "publicKey": update["public_key"], "callDepth": depth } }));
            walk(&tree["elt"]["calls"], depth + 1, out);
        }
    }
    let precomputed_zkapp = commands
        .iter()
        .find(|c| c["data"][0] == "Zkapp_command")
        .unwrap();
    let mut expected = Vec::new();
    walk(
        &precomputed_zkapp["data"][1]["account_updates"],
        0,
        &mut expected,
    );
    assert_eq!(expected.len(), 3);
    let zkapp = &block["transactions"]["zkappCommands"][0];
    assert_eq!(zkapp["zkappCommand"]["accountUpdates"], json!(expected));
    assert_eq!(zkapp["failureReason"], Value::Null);
    assert_eq!(
        zkapp["zkappCommand"]["feePayer"]["body"]["publicKey"],
        precomputed_zkapp["data"][1]["fee_payer"]["body"]["public_key"]
    );
}

/// A failed payment shows its first failure, as OCaml names it.
#[tokio::test]
async fn explorer_block_with_failed_payment() {
    let (block, precomputed) = explorer_block(
        548282,
        "3NKngFAPEubmaXEh9MdAChR6VyrzBXqzDVgHZM5eGuMzNP2S8b6N",
    )
    .await;
    let commands = precomputed_commands(&precomputed);
    let failures: Vec<_> = commands
        .iter()
        .rev()
        .map(|c| match c["status"][0].as_str().unwrap() {
            "Failed" => c["status"][1][0][0][0].clone(),
            _ => Value::Null,
        })
        .collect();
    assert_eq!(
        failures,
        [json!("Amount_insufficient_to_create_account"), Value::Null]
    );
    let shown: Vec<_> = block["transactions"]["userCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["failureReason"].clone())
        .collect();
    assert_eq!(shown, failures);
}

/// A block of two pre-diffs: the command's fees go to the coinbase receiver
/// in the first, and the second pays the snark worker from the coinbase.
/// OCaml folds the fee transfers in front of each other, so the one from the
/// coinbase, applied last, comes first.
#[tokio::test]
async fn explorer_block_with_two_pre_diffs_and_snark_work() {
    let (block, precomputed) = explorer_block(
        553961,
        "3NLUpN6t5eEcUcTXr3keAhUiK858W6kJwh1F6C6G4PXg9f5cf9R5",
    )
    .await;
    let coinbase_receiver =
        precomputed["protocol_state"]["body"]["consensus_state"]["coinbase_receiver"].clone();
    let prover = "B62qrB4hLHkwUz3UXwx6jLx6XrvbRae4d8t6pMVaGhjt2c1XoqJZTUb";
    // One work id per proof. Their values are checked against an OCaml node
    // in `work_id::tests`; this mainnet block is older than any OCaml
    // frontier we can ask.
    let works: Vec<_> = precomputed["staged_ledger_diff"]["diff"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pre_diff| !pre_diff.is_null())
        .flat_map(|pre_diff| pre_diff["completed_works"].as_array().unwrap().clone())
        .collect();
    assert_eq!(works.len(), 1);
    let proofs = works[0]["proofs"].as_array().unwrap().len() - 1;
    let jobs = block["snarkJobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["prover"], prover);
    assert_eq!(jobs[0]["fee"], "9495000");
    let work_ids = jobs[0]["workIds"].as_array().unwrap();
    assert_eq!(work_ids.len(), proofs);
    assert!(work_ids
        .iter()
        .all(|id| (0..1 << 30).contains(&id.as_i64().unwrap())));
    assert_eq!(
        block["transactions"]["feeTransfer"],
        json!([
            { "recipient": prover, "fee": "9495000", "type": "Fee_transfer_via_coinbase" },
            { "recipient": coinbase_receiver, "fee": "10000000", "type": "Fee_transfer" },
        ])
    );
    // Mesa's coinbase, not supercharged (the factor is 1).
    assert_eq!(block["transactions"]["coinbase"], "360000000000");
    assert_eq!(
        block["transactions"]["coinbaseReceiverAccount"]["publicKey"],
        coinbase_receiver
    );
    assert_eq!(
        block["protocolState"]["consensusState"]["coinbaseReceiever"],
        coinbase_receiver
    );
    assert_eq!(block["commandTransactionCount"], 1);
}

/// OCaml's failure display for a zkApp command: index 0 is the fee payer,
/// account updates without failures are left out, and the last comes first.
#[test]
fn zkapp_failure_reasons_as_ocaml() {
    use mina_p2p_messages::v2::{
        MinaBaseTransactionStatusFailureCollectionStableV1 as Collection,
        MinaBaseTransactionStatusFailureStableV2 as Failure,
    };
    let collection = Collection(
        vec![
            List::new(),
            vec![Failure::Cancelled].into_iter().collect(),
            List::new(),
            vec![
                Failure::AccountNoncePreconditionUnsatisfied,
                Failure::UpdateNotPermittedDelegate,
            ]
            .into_iter()
            .collect(),
        ]
        .into_iter()
        .collect(),
    );
    let shown: Vec<_> = super::zkapp::GraphQLFailureReason::from_collection(&collection)
        .into_iter()
        .map(|reason| (reason.index, reason.failures))
        .collect();
    assert_eq!(
        shown,
        [
            (
                "3".to_owned(),
                vec![
                    "Account_nonce_precondition_unsatisfied".to_owned(),
                    // OCaml's spelling.
                    "update_not_permitted_delegate".to_owned(),
                ]
            ),
            ("1".to_owned(), vec!["Cancelled".to_owned()]),
        ]
    );
}

#[test]
fn genesis_timestamp_as_ocaml() {
    // Mesa mainnet's genesis, 2026-09-03T18:00:00Z.
    assert_eq!(
        super::constants::genesis_timestamp_to_string(1_788_458_400_000).unwrap(),
        "2026-09-03T10:00:00.000000-08:00"
    );
}

#[test]
fn consensus_state_timestamp_as_ocaml_utc() {
    // Auro's devnet daemon reported this exact value in D2.
    assert_eq!(
        super::constants::consensus_state_timestamp_to_string(1_787_162_400_000).unwrap(),
        "2026-08-19 18:00:00.000000Z"
    );
}

/// `protocolStateProof.base64` writes a proof as precomputed blocks do:
/// OCaml's encoding of a mainnet block's proof reads back and writes the
/// same.
#[test]
fn protocol_state_proof_as_precomputed_blocks() {
    let (height, hash) = BLOCKS[0];
    let path = files().join(format!("blocks/{height}-{hash}.json.zst"));
    let bytes = zstd::decode_all(std::fs::File::open(path).unwrap()).unwrap();
    let precomputed: Value = serde_json::from_slice(&bytes).unwrap();
    let precomputed = precomputed.get("data").cloned().unwrap_or(precomputed);
    let ocaml = precomputed["protocol_state_proof"].clone();
    let proof: mina_p2p_messages::v2::PrecomputedBlockProof =
        serde_json::from_value(ocaml.clone()).unwrap();
    let ours = super::block::proof_to_base64(&proof.0).unwrap();
    assert_eq!(Value::String(ours), ocaml);
}

/// Times the conversion of blocks for `bestChain`, which computes the fee
/// transfers and coinbase of every block. Reads the blocks listed in
/// `GRAPHQL_BENCH_BLOCKS` (lines `<height> <state hash>`) from the directory
/// `GRAPHQL_BENCH_DIR`.
#[test]
#[ignore]
fn bench_block_conversion() {
    let (Ok(dir), Ok(list)) = (
        std::env::var("GRAPHQL_BENCH_DIR"),
        std::env::var("GRAPHQL_BENCH_BLOCKS"),
    ) else {
        return;
    };
    init_network();
    let mut times = Vec::new();
    for line in std::fs::read_to_string(list).unwrap().lines() {
        let mut parts = line.split_whitespace();
        let (Some(height), Some(hash)) = (parts.next(), parts.next()) else {
            continue;
        };
        let block = load_block_from(std::path::Path::new(&dir), height.parse().unwrap(), hash);
        let start = std::time::Instant::now();
        let _ = super::block::GraphQLBlock::try_from(block).unwrap();
        times.push(start.elapsed());
    }
    times.sort();
    let total: std::time::Duration = times.iter().sum();
    println!(
        "{} blocks: total {:?}, mean {:?}, median {:?}, max {:?}",
        times.len(),
        total,
        total / times.len().max(1) as u32,
        times[times.len() / 2],
        times[times.len() - 1]
    );
}

/// OCaml's genesis block has no transactions but a coinbase to the genesis
/// winner, as in its response for Mesa mainnet's genesis. Any block stands
/// in for the genesis block here.
#[tokio::test]
async fn genesis_block_transactions_as_ocaml() {
    let ocaml = ocaml_response("GenesisBlock");
    let ocaml = &ocaml["data"]["genesisBlock"];
    assert_eq!(ocaml["transactions"]["feeTransfer"], json!([]));
    assert_eq!(ocaml["snarkJobs"], json!([]));
    assert_eq!(
        ocaml["transactions"]["coinbaseReceiverAccount"]["publicKey"],
        ocaml["protocolState"]["consensusState"]["blockCreator"]
    );

    let mut node = best_chain_node();
    node.genesis = Some(node.best_chain[0].block.clone());
    let response = execute(
        node,
        "GenesisBlock",
        &sdk_operation("GenesisBlock"),
        json!({}),
    )
    .await;
    assert_eq!(response.get("errors"), None);
    let ours = &response["data"]["genesisBlock"];
    let creator = &ours["protocolState"]["consensusState"]["blockCreator"];
    assert_eq!(
        ours["transactions"],
        json!({
            "coinbase": ocaml["transactions"]["coinbase"],
            "coinbaseReceiverAccount": { "publicKey": creator },
            "feeTransfer": [],
            "userCommands": [],
            "zkappCommands": [],
        })
    );
    assert_eq!(ours["snarkJobs"], json!([]));
    assert_eq!(ours["commandTransactionCount"], 0);
    assert_eq!(ours["creatorAccount"]["publicKey"], *creator);
}

/// OCaml looks every block's `stagedLedgerProofEmitted` up in the transition
/// frontier and gives null when it is not there: R32's OCaml devnet node
/// answered null for the Mesa fork genesis (height 545434), which no
/// frontier holds. A genesis block still in the frontier gives its flag.
#[tokio::test]
async fn genesis_staged_ledger_proof_emitted_from_frontier() {
    const QUERY: &str = "query G { genesisBlock { stateHash protocolState { blockchainState { stagedLedgerProofEmitted } } } }";
    let emitted = |response: &Value| {
        assert_eq!(response.get("errors"), None);
        response["data"]["genesisBlock"]["protocolState"]["blockchainState"]
            ["stagedLedgerProofEmitted"]
            .clone()
    };

    // A genesis block outside the frontier.
    let mut node = best_chain_node();
    let outside = node.best_chain.remove(0);
    node.genesis = Some(outside.block.clone());
    assert_eq!(
        emitted(&execute(node, "G", QUERY, json!({})).await),
        Value::Null
    );

    // The genesis block is the frontier's root.
    let mut node = best_chain_node();
    node.genesis = Some(node.best_chain[0].block.clone());
    assert_eq!(
        emitted(&execute(node, "G", QUERY, json!({})).await),
        json!(false)
    );
    let mut node = best_chain_node();
    node.best_chain[0].just_emitted_a_proof = true;
    node.genesis = Some(node.best_chain[0].block.clone());
    assert_eq!(
        emitted(&execute(node, "G", QUERY, json!({})).await),
        json!(true)
    );
}

/// Executes a request through the HTTP handler's pipeline, as a client
/// sees it.
async fn execute_http(node: FakeNode, request: Value) -> (u16, Value) {
    respond_http(node.spawn(), request, false).await
}

async fn respond_http(rpc: RpcSender, request: Value, public: bool) -> (u16, Value) {
    let request: juniper::http::GraphQLBatchRequest = serde_json::from_value(request).unwrap();
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(rpc).with_public_graphql(public);
    let (status, response) =
        crate::http_server::routes::graphql::respond(&schema, &context, &request, public).await;
    (status.as_u16(), response)
}

/// The public policy's answer to a request: `Ok` with the response when
/// it lets the request through (to a fake node), `Err` with the refusal's
/// code when it refuses it, which must be before any RPC.
async fn public(request: Value) -> Result<Value, String> {
    let batch: juniper::http::GraphQLBatchRequest =
        serde_json::from_value(request.clone()).unwrap();
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    if let Some(code) = crate::http_server::routes::graphql::public_refusal(&batch, &schema) {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let (status, response) = respond_http(RpcSender::new(tx), request, true).await;
        assert_eq!(status, 400);
        assert_eq!(response["errors"][0]["extensions"]["code"], code);
        assert!(
            rx.try_recv().is_err(),
            "a refused request must not reach the node"
        );
        return Err(code.to_owned());
    }
    // Let it through: answer it from the fake node.
    let (status, response) = respond_http(best_chain_node().spawn(), request, true).await;
    assert_eq!(status, 200);
    Ok(response)
}

/// R32's malformed requests get OCaml's responses: its messages, no
/// `locations`, and argument errors (a public key that does not decode, a
/// variable left out) as the whole response with `data: null`.
#[tokio::test]
async fn malformed_requests_answered_as_ocaml() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(files().join("ocaml/r32-errors.json")).unwrap(),
    )
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (status, response) = execute_http(
            best_chain_node(),
            json!({ "query": case["query"], "variables": case["variables"] }),
        )
        .await;
        assert_eq!(status, 200, "{name}");
        assert_json_eq(&response, &case["ocaml"], name);
    }
}

/// OCaml fails on any variable an executed argument uses that the request
/// leaves out and that has no default; given as null, or with a default,
/// it is fine. Documents Juniper refuses keep Juniper's errors.
#[tokio::test]
async fn missing_variables_as_ocaml() {
    let key = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
    let query = "query A($publicKey: PublicKey!, $token: TokenId) { account(publicKey: $publicKey, token: $token) { nonce } }";
    let missing = |name: &str| json!({ "errors": [{ "message": format!("Missing variable `{name}`") }], "data": null });

    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": query, "variables": { "publicKey": key, "token": null } }),
    )
    .await;
    assert_eq!(response, json!({ "data": { "account": null } }));
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": query, "variables": {} }),
    )
    .await;
    assert_eq!(response, missing("publicKey"));
    let (_, response) = execute_http(best_chain_node(), json!({ "query": query })).await;
    assert_eq!(response, missing("publicKey"));

    // Through a fragment and a directive.
    let query = "query B($show: Boolean!, $h: Int) { ...F @include(if: $show) } fragment F on Query { block(height: $h) { stateHash } }";
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": query, "variables": {} }),
    )
    .await;
    assert_eq!(response, missing("show"));
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": query, "variables": { "show": true } }),
    )
    .await;
    assert_eq!(response, missing("h"));

    // A default stands in for the variable. (OCaml also takes a declared
    // variable no argument uses; Juniper's validation refuses it.)
    let query = "query C($h: Int = 564494) { block(height: $h) { stateHash } }";
    let (_, response) = execute_http(best_chain_node(), json!({ "query": query })).await;
    assert_eq!(
        response,
        json!({ "data": { "block": { "stateHash": BLOCKS[1].1 } } })
    );

    // An unknown field is a validation error first.
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": "query D($h: Int) { block(height: $h) { nope } }" }),
    )
    .await;
    assert!(response["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("nope"));
    assert_eq!(response["errors"][0].get("locations"), None);
}

/// The block lookups' messages are those of OCaml's `Mina_lib`.
#[tokio::test]
async fn block_not_found_as_ocaml() {
    let hash = "3NLhyJ6wdC6qM7MoJwMK6fWQf3ZNKMNbqUtPbgmgTYbEXNYjEP1i";
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": format!("{{ block(stateHash: \"{hash}\") {{ stateHash }} }}") }),
    )
    .await;
    assert_eq!(
        response,
        json!({ "errors": [{ "message": format!("Block with state hash {hash} not found in transition frontier"), "path": ["block"] }], "data": null })
    );
    let (_, response) = execute_http(
        best_chain_node(),
        json!({ "query": "{ block(height: -1) { stateHash } }" }),
    )
    .await;
    assert_eq!(
        response,
        json!({ "errors": [{ "message": "Could not find block in transition frontier with height 4294967295", "path": ["block"] }], "data": null })
    );
}

/// `tokenOwner`: the default token has no owner (R32's OCaml devnet node
/// answered null to Auro's `tokenInfo`, in the R32 fixture above), with no
/// ledger lookup; another token's owner is the account whose id derives it.
/// The derivation is the consensus code token accounts are created with,
/// checked against OCaml by replaying blocks; this checks the lookup.
#[tokio::test]
async fn token_owner_as_ocaml() {
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(RpcSender::new(tx));
    let request: GraphQLRequest = serde_json::from_value(json!({
        "query": "{ tokenOwner(tokenId: \"wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf\") { publicKey } }"
    }))
    .unwrap();
    let response = serde_json::to_value(request.execute(&schema, &context).await).unwrap();
    assert_eq!(response, json!({ "data": { "tokenOwner": null } }));
    assert!(rx.try_recv().is_err(), "the default token needs no lookup");

    let mut node = best_chain_node();
    let public_key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let owner = AccountId::new_with_default_token(
        mina_signer::CompressedPubKey::from_address(public_key).unwrap(),
    );
    let token = mina_p2p_messages::v2::TokenIdKeyHash::from(owner.derive_token_id()).to_string();
    node.accounts
        .push(Account::create_with(owner, Balance::from_u64(1)));
    let query = "query T($t: TokenId!) { tokenOwner(tokenId: $t) { publicKey tokenId } }";
    let response = execute(node.clone(), "T", query, json!({ "t": token })).await;
    assert_eq!(
        response,
        json!({ "data": { "tokenOwner": { "publicKey": public_key, "tokenId": "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf" } } })
    );
    // A token nobody's id derives.
    let other =
        mina_p2p_messages::v2::TokenIdKeyHash::from(ledger::TokenId::from(2u64)).to_string();
    let response = execute(node.clone(), "T", query, json!({ "t": other })).await;
    assert_eq!(response, json!({ "data": { "tokenOwner": null } }));
    let (_, response) = execute_http(
        node,
        json!({ "query": "{ tokenOwner(tokenId: \"nope\") { publicKey } }" }),
    )
    .await;
    assert_eq!(
        response,
        json!({ "errors": [{ "message": "Argument `tokenId` of type `TokenId` expected on field `tokenOwner`, found \"nope\"." }], "data": null })
    );
}

/// A checksummed token id whose value is outside the field is refused as an
/// argument (the decoder refuses it), before any ledger lookup.
#[tokio::test]
async fn out_of_field_token_id_is_an_argument_error() {
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(RpcSender::new(tx));
    let token = mina_p2p_messages::v2::TokenIdKeyHash::from(
        mina_p2p_messages::v2::MinaBaseAccountIdDigestStableV1(
            mina_p2p_messages::bigint::BigInt::from_bytes([0xff; 32]),
        ),
    )
    .to_string();
    for (query, field, arg) in [
        (
            "query T($t: TokenId!) { tokenOwner(tokenId: $t) { publicKey } }",
            "tokenOwner",
            "tokenId",
        ),
        (
            "query T($t: TokenId!) { account(publicKey: \"B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U\", token: $t) { publicKey } }",
            "account",
            "token",
        ),
    ] {
        let request: GraphQLRequest = serde_json::from_value(json!({
            "query": query,
            "operationName": "T",
            "variables": { "t": token },
        }))
        .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            request.execute(&schema, &context),
        )
        .await
        .expect("answered without a ledger lookup");
        let response = serde_json::to_value(response).unwrap();
        assert_eq!(
            response["errors"][0]["message"],
            format!("Argument `{arg}` of type `TokenId` expected on field `{field}`, found \"{token}\"."),
            "{response}"
        );
        assert!(rx.try_recv().is_err());
    }
}

/// The grant frontend's three documents, as `chain.service.ts` sends them.
fn frontend_operations() -> Vec<(&'static str, String, Value)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../frontend/src/app/features/chain/chain.service.ts");
    let source = std::fs::read_to_string(path).unwrap();
    let start = source.find("export const BLOCK_FIELDS = `").unwrap() + 29;
    let fields = &source[start..start + source[start..].find('`').unwrap()];
    let account = &source[source.find("query LedgerAccount").unwrap()..];
    let account = &account[..account.find('`').unwrap()];
    vec![
        (
            "RecentBlocks",
            format!("query RecentBlocks($maxLength: Int!) {{ bestChain(maxLength: $maxLength) {{ {fields} }} }}"),
            json!({ "maxLength": 20 }),
        ),
        (
            "BlockDetails",
            format!("query BlockDetails($stateHash: String!) {{ block(stateHash: $stateHash) {{ {fields} }} }}"),
            json!({ "stateHash": BLOCKS[0].1 }),
        ),
        (
            "LedgerAccount",
            account.to_owned(),
            json!({ "publicKey": "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U" }),
        ),
    ]
}

/// What `--public-graphql` lets through: the grant frontend's documents and
/// the bounded read documents of o1js and the Auro wallet. The two Auro
/// documents that require full-ledger scans are private-only.
#[tokio::test]
async fn public_policy_takes_client_reads() {
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let mut documents = frontend_operations();
    for (_, name, text) in client_operations("o1js.graphql") {
        documents.push(("o1js", text, json!({})));
        let _ = name;
    }
    for (_, name, text) in client_operations("auro.graphql") {
        if matches!(name.as_str(), "tokenQueryBody" | "tokenInfo") {
            assert_eq!(
                public(json!({ "query": text, "variables": { "publicKey": key } })).await,
                Err("PUBLIC_GRAPHQL_NOT_ALLOWED".to_owned()),
                "{name} must not scan the ledger in public mode",
            );
            continue;
        }

        let variables = match name.as_str() {
            "txStatus" => json!({ "paymentId": "invalid" }),
            "batchBalance" => json!({ "account0": key, "account1": key }),
            "blockInfo" => json!({ "stateHash": BLOCKS[0].1 }),
            "tokenState" => {
                json!({ "publicKey": key, "tokenId": "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf" })
            }
            _ => json!({ "publicKey": key }),
        };
        documents.push(("auro", text, variables));
    }
    assert_eq!(documents.len(), 3 + 5 + 9);
    for (client, query, variables) in documents {
        let response = public(json!({ "query": query, "variables": variables }))
            .await
            .unwrap_or_else(|code| panic!("{client} refused ({code}): {query}"));
        assert!(response.get("data").is_some(), "{client}: {response}");
    }
}

/// What it refuses, before any RPC: writes, batches, introspection, peers
/// and operator fields, unbounded or oversized lists, and deep, wide or
/// costly queries.
#[tokio::test]
async fn public_policy_refuses_before_rpc() {
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let refused = |code: &str| Err::<Value, String>(code.to_owned());
    let not_allowed = refused("PUBLIC_GRAPHQL_NOT_ALLOWED");
    let too_complex = refused("PUBLIC_GRAPHQL_TOO_COMPLEX");
    let bad_argument = refused("PUBLIC_GRAPHQL_BAD_ARGUMENT");
    let one = |query: &str| json!({ "query": query });

    for (send, expected) in client_operations("sends.graphql")
        .into_iter()
        .map(|(_, _, text)| (one(&text), &not_allowed))
        .chain([
            (one(&format!("{{ accounts(publicKey: \"{key}\") {{ publicKey }} }}")), &not_allowed),
            (one("{ tokenOwner(tokenId: \"wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf\") { publicKey } }"), &not_allowed),
            (one("{ getPeers { host } }"), &not_allowed),
            (one("{ daemonStatus { peers { host } } }"), &not_allowed),
            (one("{ daemonStatus { addrsAndPorts { externalIp } } }"), &not_allowed),
            (one("{ version }"), &not_allowed),
            (one("{ snarkPool { fee } }"), &not_allowed),
            (one("{ currentSnarkWorker { key } }"), &not_allowed),
            (one("{ verification { capacity } }"), &not_allowed),
            (one("{ trackedAccounts { publicKey } }"), &not_allowed),
            (one("{ __schema { types { name } } }"), &not_allowed),
            (one("{ __type(name: \"Block\") { name } }"), &not_allowed),
            (one("{ block(height: 1) { protocolStateProof { base64 } } }"), &not_allowed),
            (one(&format!("{{ account(publicKey: \"{key}\") {{ delegators {{ publicKey }} }} }}")), &not_allowed),
            (one("fragment F on Query { getPeers { host } } { ...F }"), &not_allowed),
            (one("{ ... on Query { initialPeers } }"), &not_allowed),
            (one("query A { syncStatus } query B { networkID }"), &not_allowed),
            (json!([{ "query": "{ syncStatus }" }, { "query": "{ networkID }" }]), &not_allowed),
            (one("{ bestChain { stateHash } }"), &bad_argument),
            (one("{ bestChain(maxLength: 21) { stateHash } }"), &bad_argument),
            (one("{ bestChain(maxLength: 0) { stateHash } }"), &bad_argument),
            (json!({ "query": "query($n: Int) { bestChain(maxLength: $n) { stateHash } }", "variables": { "n": 300 } }), &bad_argument),
            (json!({ "query": "query($n: Int) { bestChain(maxLength: $n) { stateHash } }" }), &bad_argument),
            (one("{ pooledUserCommands { hash } }"), &bad_argument),
            (json!({ "query": "query($h: [String!]) { pooledZkappCommands(hashes: $h) { hash } }", "variables": { "h": vec!["x"; 11] } }), &bad_argument),
            // 20 blocks, each with 128 user commands loading 2 accounts.
            (one("{ bestChain(maxLength: 20) { transactions { userCommands { source { nonce } receiver { nonce } } } } }"), &too_complex),
            (one(&format!("{{ {} }}", (0..11).map(|i| format!("s{i}: syncStatus")).collect::<Vec<_>>().join(" "))), &too_complex),
            (one(&format!("{{ account(publicKey: \"{key}\") {{ {}publicKey{} }} }}", "delegateAccount { ".repeat(12), " }".repeat(12))), &too_complex),
            // 20 blocks' zkApp commands with their proofs.
            (one("{ bestChain(maxLength: 20) { transactions { zkappCommands { zkappCommand { accountUpdates { authorization { proof } } } } } } }"), &too_complex),
        ])
    {
        assert_eq!(&public(send.clone()).await, expected, "{send}");
    }

    // The same requests pass without the policy (or are refused by Juniper,
    // not by it).
    let (status, _) = respond_http(
        best_chain_node().spawn(),
        one("{ bestChain { stateHash } }"),
        false,
    )
    .await;
    assert_eq!(status, 200);
    // `__typename` and fields the schema lacks are Juniper's to answer.
    let typename = public(one("{ __typename syncStatus }")).await.unwrap();
    assert_eq!(typename["data"]["__typename"], "Query");
    assert_eq!(typename.get("errors"), None);
    let unknown = public(one("{ nope }")).await.unwrap();
    assert!(unknown["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("nope"));
}

/// The public dashboard's receipt lookup, as `verification.types.ts` sends it.
fn frontend_receipts_query() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../frontend/src/app/features/verification/verification.types.ts");
    let source = std::fs::read_to_string(path).unwrap();
    let marker = "export const PUBLIC_RECEIPTS_QUERY = `";
    let start = source.find(marker).unwrap() + marker.len();
    source[start..start + source[start..].find('`').unwrap()].to_owned()
}

/// `verificationReceipts`: up to 20 state hashes, answered with the fixed
/// public receipt in the order given, blocks without a receipt left out.
/// More hashes are refused before any RPC in public mode and by the
/// resolver, also before any RPC, in private mode.
#[tokio::test]
async fn public_receipts_are_bounded_and_fixed() {
    let query = frontend_receipts_query();
    let hashes = |n: usize| -> Vec<&str> { BLOCKS.iter().map(|b| b.1).cycle().take(n).collect() };

    // The dashboard's document, with the 20 hashes of a page of blocks.
    let response = public(json!({ "query": query, "variables": { "stateHashes": hashes(20) } }))
        .await
        .unwrap();
    assert_eq!(response.get("errors"), None, "{response}");
    let receipts = response["data"]["verificationReceipts"].as_array().unwrap();
    // BLOCKS[0] has no receipt; repeats are answered once, in order.
    assert_eq!(
        receipts
            .iter()
            .map(|r| r["stateHash"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![BLOCKS[1].1, BLOCKS[2].1]
    );
    let check = |elapsed: Value| json!({ "status": "passed", "elapsedMs": elapsed });
    assert_eq!(
        receipts[0],
        json!({
            "stateHash": BLOCKS[1].1,
            "height": "564494",
            "network": "mainnet",
            "arrival": "gossip",
            "applied": true,
            "blockProof": check(json!(1500.5)),
            "deltaChain": check(Value::Null),
            "bodyReference": check(Value::Null),
            "verifierKey": "ab".repeat(32),
        })
    );

    // The public types are named field by field in the allow-list, so a
    // field added to them is refused until it is listed.
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(best_chain_node().spawn());
    let (value, errors) =
        juniper::introspect(&schema, &context, juniper::IntrospectionFormat::default()).unwrap();
    assert!(errors.is_empty());
    let value = serde_json::to_value(value).unwrap();
    let fields = |name: &str| -> Vec<String> {
        let ty = value["__schema"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name}"));
        let mut names: Vec<String> = ty["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        fields("PublicVerificationReceipt"),
        [
            "applied",
            "arrival",
            "blockProof",
            "bodyReference",
            "deltaChain",
            "height",
            "network",
            "stateHash",
            "verifierKey",
        ]
    );
    assert_eq!(fields("PublicVerificationCheck"), ["elapsedMs", "status"]);
    // This fixture records Headers first: both header check times equal
    // received_at. Neither public check exposes an absolute timestamp.
    let (_, private) = respond_http(
        best_chain_node().spawn(),
        json!({ "query": "{ verification { receipts { stateHash receivedAt deltaChain { checkedAt } bodyReference { checkedAt } blockProof { checkedAt } } } }" }),
        false,
    )
    .await;
    for receipt in private["data"]["verification"]["receipts"]
        .as_array()
        .unwrap()
    {
        let projected = receipts
            .iter()
            .find(|r| r["stateHash"] == receipt["stateHash"])
            .unwrap();
        for field in ["deltaChain", "bodyReference", "blockProof"] {
            assert_eq!(receipt[field]["checkedAt"], receipt["receivedAt"]);
            assert!(projected[field].get("checkedAt").is_none());
            assert_ne!(projected[field]["checkedAt"], receipt["receivedAt"]);
            let request = json!({ "query": format!("{{ verificationReceipts(stateHashes: [\"{}\"]) {{ {field} {{ checkedAt }} }} }}", BLOCKS[1].1) });
            assert!(public(request).await.unwrap()["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("checkedAt"));
        }
    }

    // The private receipt's other fields are not in the public type.
    for private in ["receivedAt", "capacity", "recentFailures", "lastVerified"] {
        let request = one_receipt_field(private);
        assert!(
            public(request.clone()).await.unwrap()["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains(private),
            "{request}"
        );
    }

    // 21 hashes: literal, variable and default, refused before any RPC.
    let many = serde_json::to_string(&hashes(21)).unwrap();
    for request in [
        json!({ "query": query, "variables": { "stateHashes": hashes(21) } }),
        json!({ "query": format!("{{ verificationReceipts(stateHashes: {many}) {{ stateHash }} }}") }),
        json!({ "query": format!("query($h: [String!]! = {many}) {{ verificationReceipts(stateHashes: $h) {{ stateHash }} }}") }),
    ] {
        assert_eq!(
            public(request.clone()).await,
            Err("PUBLIC_GRAPHQL_BAD_ARGUMENT".into()),
            "{request}"
        );
    }
    // The private `verification` field stays private.
    assert_eq!(
        public(json!({ "query": "{ verification { receipts { receivedAt } } }" })).await,
        Err("PUBLIC_GRAPHQL_NOT_ALLOWED".into())
    );

    // Private mode: the same field and bound, enforced by the resolver
    // with no RPC; an invalid hash is an error, an empty list needs no RPC.
    for (state_hashes, error) in [
        (json!(hashes(21)), Some("at most 20 state hashes, got 21")),
        (json!(["nope"]), Some("Invalid state hash nope")),
        (json!([]), None),
    ] {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let (status, response) = respond_http(
            RpcSender::new(tx),
            json!({ "query": query, "variables": { "stateHashes": state_hashes } }),
            false,
        )
        .await;
        assert_eq!(status, 200);
        match error {
            Some(error) => assert!(
                response["errors"][0]["message"]
                    .as_str()
                    .unwrap()
                    .contains(error),
                "{response}"
            ),
            None => assert_eq!(response, json!({ "data": { "verificationReceipts": [] } })),
        }
        assert!(rx.try_recv().is_err(), "no lookup for {state_hashes}");
    }
    // The private receipts and counters are unchanged.
    let (status, response) = respond_http(
        best_chain_node().spawn(),
        json!({ "query": "{ verification { verifiedSinceStart receipts(limit: 5) { stateHash receivedAt arrival } } }" }),
        false,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        response,
        json!({ "data": { "verification": {
            "verifiedSinceStart": "2",
            "receipts": [
                { "stateHash": BLOCKS[2].1, "receivedAt": "1700000002000", "arrival": "gossip" },
                { "stateHash": BLOCKS[1].1, "receivedAt": "1700000001000", "arrival": "gossip" },
            ],
        } } })
    );
}

fn one_receipt_field(field: &str) -> Value {
    json!({ "query": format!("{{ verificationReceipts(stateHashes: [\"{}\"]) {{ {field} }} }}", BLOCKS[1].1) })
}

#[tokio::test]
async fn public_receipt_budget_is_request_wide() {
    // Twenty devnet hashes from the original live receipt capture documented
    // in private implementation notes, plus an OCaml mainnet fixture hash.
    let hashes: Vec<String> = serde_json::from_str(include_str!(
        "../../tests/files/graphql/receipt-hashes.json"
    ))
    .unwrap();
    let mut distinct: std::collections::BTreeSet<_> = hashes.iter().map(String::as_str).collect();
    assert_eq!(distinct.len(), 20);
    assert!(distinct.insert(BLOCKS[1].1));
    let first = serde_json::to_string(&hashes).unwrap();
    let second = serde_json::to_string(&[BLOCKS[1].1]).unwrap();
    let aliases = "a: verificationReceipts(stateHashes: $a) { stateHash } b: verificationReceipts(stateHashes: $b) { stateHash }";
    let ten = (0..10)
        .map(|i| format!("r{i}: verificationReceipts(stateHashes: $h) {{ stateHash }}"))
        .collect::<Vec<_>>()
        .join(" ");
    let requests = [
        // 21 distinct hashes split as 20 + 1; effective variables/defaults
        // and both forms of fragments obey the same request-wide budget.
        json!({ "query": format!("{{ a: verificationReceipts(stateHashes: {first}) {{ stateHash }} b: verificationReceipts(stateHashes: {second}) {{ stateHash }} }}") }),
        json!({ "query": format!("query($a: [String!]!, $b: [String!]!) {{ {aliases} }}"), "variables": { "a": hashes, "b": [BLOCKS[1].1] } }),
        json!({ "query": format!("query($a: [String!]! = {first}, $b: [String!]! = {second}) {{ ...R }} fragment R on Query {{ {aliases} }}") }),
        json!({ "query": format!("query($a: [String!]!, $b: [String!]!) {{ ... on Query {{ {aliases} }} }}"), "variables": { "a": hashes, "b": [BLOCKS[1].1] } }),
        // Even a single distinct hash may not trigger ten lookups.
        json!({ "query": format!("query($h: [String!]!) {{ {ten} }}"), "variables": { "h": [BLOCKS[1].1] } }),
        json!({ "query": format!("query($h: [String!]!) {{ ...R }} fragment R on Query {{ {ten} }}"), "variables": { "h": hashes } }),
        // Repeated spreads and skipped fields count conservatively, just as
        // in R34's cost walker, even when Juniper could merge/skip them.
        json!({ "query": "{ ...R ...R } fragment R on Query { verificationReceipts(stateHashes: []) { stateHash } }" }),
        json!({ "query": "{ a: verificationReceipts(stateHashes: []) { stateHash } b: verificationReceipts(stateHashes: []) @skip(if: true) { stateHash } }" }),
    ];
    for request in requests {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let (status, response) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            respond_http(RpcSender::new(tx), request.clone(), true),
        )
        .await
        .unwrap_or_else(|_| panic!("request reached a resolver: {request}"));
        assert_eq!(status, 400, "{request}: {response}");
        assert_eq!(
            response["errors"][0]["extensions"]["code"], "PUBLIC_GRAPHQL_BAD_ARGUMENT",
            "{request}: {response}"
        );
        assert!(rx.try_recv().is_err(), "no work may run: {request}");
    }

    // One aliased selection inside nested fragments still accepts 20
    // distinct valid hashes, makes exactly one RPC, and preserves order.
    init_network();
    let mut state = VerificationState::default();
    for hash in &hashes {
        state.record(
            &VerificationUpdate::Headers {
                block: BlockIdentity {
                    state_hash: hash.parse().unwrap(),
                    height: 578098,
                },
                arrival: mina_node::verification::Arrival::Gossip,
                delta: CheckStatus::Passed,
                body: CheckStatus::Passed,
            },
            redux::Timestamp::new(1_700_000_000_000_000_000),
        );
    }
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
    let expected = hashes.clone();
    let server = tokio::spawn(async move {
        let mut lookups = 0;
        while let Some(NodeRpcRequest { req, responder }) = rx.recv().await {
            let RpcRequest::VerificationStatsGet(query) = req else {
                panic!("unexpected RPC");
            };
            let snapshot = state.snapshot(query);
            assert_eq!(snapshot.lookup.as_ref().unwrap().requested, 20);
            assert_eq!(
                snapshot
                    .receipts
                    .iter()
                    .map(|r| r.block.state_hash.to_string())
                    .collect::<Vec<_>>(),
                expected
            );
            respond::<RpcVerificationStatsGetResponse>(responder, snapshot);
            lookups += 1;
        }
        lookups
    });
    let (status, response) = respond_http(
        RpcSender::new(tx),
        json!({ "query": "query($h: [String!]!) { ...Outer } fragment Outer on Query { ... on Query { ...Inner } } fragment Inner on Query { page: verificationReceipts(stateHashes: $h) { stateHash } }", "variables": { "h": hashes } }),
        true,
    ).await;
    assert_eq!(status, 200);
    assert_eq!(response.get("errors"), None, "{response}");
    assert_eq!(response["data"]["page"].as_array().unwrap().len(), 20);
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn public_pool_bounds_include_keys_variables_and_non_null_lists() {
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    for pool in ["pooledUserCommands", "pooledZkappCommands"] {
        for argument in ["hashes", "ids"] {
            // Literal, variable and operation-default lists all obey the cap,
            // independently of the public-key filter and the other list.
            let ids = serde_json::to_string(&vec!["x"; 11]).unwrap();
            for request in [
                json!({"query": format!("{{ {pool}(publicKey: \"{key}\", {argument}: {ids}) {{ hash }} }}")}),
                json!({"query": format!("query($ids: [String!]) {{ {pool}(publicKey: \"{key}\", {argument}: $ids) {{ hash }} }}"), "variables": {"ids": vec!["x"; 11]}}),
                json!({"query": format!("query($ids: [String!] = {ids}) {{ {pool}(publicKey: \"{key}\", {argument}: $ids) {{ hash }} }}")}),
            ] {
                assert_eq!(
                    public(request.clone()).await,
                    Err("PUBLIC_GRAPHQL_BAD_ARGUMENT".into()),
                    "{request}"
                );
            }
        }
    }
    // [GraphQLAccountUpdate!]! must multiply all descendant proofs by eight.
    // Pool cost 30 + 10 commands * 8 updates * 12 proofs = 990 (accepted);
    // one more proof alias costs 1070 and must be refused before any RPC.
    for (count, accepted) in [(12, true), (13, false)] {
        let proofs = (0..count)
            .map(|n| format!("p{n}: proof"))
            .collect::<Vec<_>>()
            .join(" ");
        let request = json!({"query": format!("{{ pooledZkappCommands(publicKey: \"{key}\") {{ zkappCommand {{ accountUpdates {{ authorization {{ {proofs} }} }} }} }} }}")});
        let result = public(request).await;
        if accepted {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert_eq!(result, Err("PUBLIC_GRAPHQL_TOO_COMPLEX".into()));
        }
    }
}

/// The two SDK sends are in the 45-operation check above. Auro's four send
/// variants were excluded from P7's original corpus; cover them separately.
#[tokio::test]
async fn wallet_send_operations_validate() {
    assert_eq!(client_operations("sends.graphql").len(), 4);
    assert_eq!(validation_errors("sends.graphql").await, Vec::new());
}

/// Field names, scalar names, nullability, interface membership and concrete
/// types, projected from OCaml's graphql_schema.json. The 4.0.2 source is
/// types.ml:1771-2005,2339-2355, not our own schema output.
#[tokio::test]
async fn send_schema_matches_ocaml() {
    fn type_name(ty: &Value) -> String {
        match ty["name"].as_str() {
            Some(name) => format!("{}:{name}", ty["kind"].as_str().unwrap()),
            None => format!(
                "{}({})",
                ty["kind"].as_str().unwrap(),
                type_name(&ty["ofType"])
            ),
        }
    }
    fn names(types: &Value) -> Vec<String> {
        let mut names: Vec<_> = types
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(best_chain_node().spawn());
    let (value, errors) =
        juniper::introspect(&schema, &context, juniper::IntrospectionFormat::default()).unwrap();
    assert!(errors.is_empty());
    let schema = serde_json::to_value(value).unwrap();
    let expected = ocaml_response("SendSchema");
    let mut actual = serde_json::Map::new();
    for ty in schema["__schema"]["types"].as_array().unwrap() {
        let name = ty["name"].as_str().unwrap();
        if expected.get(name).is_none() {
            continue;
        }
        let fields: serde_json::Map<String, Value> = ty["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().to_owned(),
                    json!(type_name(&f["type"])),
                )
            })
            .collect();
        actual.insert(
            name.to_owned(),
            json!({
                "kind":ty["kind"], "fields":fields,
                "possibleTypes":names(&ty["possibleTypes"]), "interfaces":names(&ty["interfaces"]),
            }),
        );
    }
    assert_json_eq(&Value::Object(actual), &expected, "send schema");
}

/// Rosetta's current source has one stale send operation: payment still
/// supplies token and a UInt64 validUntil. OCaml's schema rejects it too;
/// preserve the source operation and do not add non-OCaml arguments to pass it.
#[tokio::test]
async fn rosetta_send_operations_have_ocaml_validation_outcomes() {
    assert_eq!(client_operations("rosetta-sends.graphql").len(), 2);
    assert_eq!(
        invalid_operations("rosetta-sends.graphql").await,
        ["RosettaSendPayment"]
    );
}

/// A node status with the given block producer and next production, the
/// rest empty.
fn producer_status(
    producer: Option<&str>,
    next_block_production: Option<RpcNextBlockProduction>,
) -> RpcNodeStatus {
    let sync = RpcNodeStatusTransitionFrontierSync {
        time: None,
        status: String::new(),
        phase: String::new(),
        target: None,
    };
    RpcNodeStatus {
        network: "devnet".to_owned(),
        uptime_secs: 0,
        chain_id: None,
        transition_frontier: RpcNodeStatusTransitionFrontier {
            root: None,
            best_tip: None,
            sync,
        },
        ledger: RpcNodeStatusLedger {
            alive_masks_after_last_commit: 0,
            pending_writes: vec![],
            pending_reads: vec![],
        },
        snark_pool: Default::default(),
        transaction_pool: Default::default(),
        current_block_production_attempt: None,
        previous_block_production_attempt: None,
        peers: vec![],
        resources_status: RpcNodeStatusResources {
            memory_debug: None,
            p2p_malloc_size: 0,
            transition_frontier: Value::Null,
            snark_pool: Value::Null,
        },
        service_queues: serde_json::from_value(json!({
            "events": 0, "snark_block_verify": 0, "ledger": 0, "vrf_evaluator": null,
            "block_prover": null, "p2p_webrtc": 0, "p2p_libp2p": 0, "rpc": 0,
        }))
        .unwrap(),
        network_info: RpcNodeStatusNetworkInfo {
            bind_ip: "0.0.0.0".to_owned(),
            external_ip: None,
            client_port: None,
            libp2p_port: None,
        },
        block_producer: producer.map(|key| key.parse().unwrap()),
        coinbase_receiver: producer.map(|key| key.parse().unwrap()),
        next_block_production,
    }
}

const NEXT_BLOCK_PRODUCTION: &str = "query N { daemonStatus { nextBlockProduction { \
    times { epoch slot globalSlot startTime endTime } globalSlotSinceGenesis \
    generatedFromConsensusAt { globalSlotSinceGenesis \
    consensusTime { epoch slot globalSlot startTime endTime } } } } }";

/// `daemonStatus.nextBlockProduction`, as OCaml's `block_producer_timing`
/// shows `Next_producer_timing` (`types.ml` L145-L206): null on a node
/// without a block producer (the public OCaml devnet node's answer, in
/// `epoch-consensus-time-devnet.json`); with one, the won slot it produces
/// in next, or empty lists while it has none (`Evaluating_vrf`,
/// `Check_again`), and the best tip's slot. Times are OCaml's `BlockTime`:
/// milliseconds since the Unix epoch, as strings. The devnet best tip is
/// the OCaml node's (`consensusTimeBestTip`, `globalSlotSinceGenesisBestTip`).
#[tokio::test]
async fn next_block_production_as_ocaml() {
    let ocaml = ocaml_response("epoch-consensus-time-devnet");
    let response = execute(best_chain_node(), "N", NEXT_BLOCK_PRODUCTION, json!({})).await;
    assert_eq!(
        response["data"]["daemonStatus"]["nextBlockProduction"],
        ocaml["data"]["daemonStatus"]["nextBlockProduction"]
    );
    assert_eq!(
        response,
        json!({ "data": { "daemonStatus": { "nextBlockProduction": null } } })
    );

    let genesis_ms = 1_787_162_400_000;
    let slot = |global_slot, since_genesis| {
        RpcConsensusSlot::new(global_slot, since_genesis, 7140, genesis_ms, 90_000).unwrap()
    };
    let best_tip = ocaml["data"]["daemonStatus"]["consensusTimeBestTip"].clone();
    let generated = json!({
        "globalSlotSinceGenesis": "920220",
        "consensusTime": best_tip,
    });
    let producer = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
    let node = |next_won_slot| FakeNode {
        status: Some(producer_status(
            Some(producer),
            Some(RpcNextBlockProduction {
                generated_from_consensus_at: slot(60660, 920220),
                next_won_slot,
            }),
        )),
        ..best_chain_node()
    };

    let response = execute(
        node(Some(slot(60700, 920260))),
        "N",
        NEXT_BLOCK_PRODUCTION,
        json!({}),
    )
    .await;
    assert_eq!(
        response,
        json!({ "data": { "daemonStatus": { "nextBlockProduction": {
            "times": [{
                "epoch": "8",
                "slot": "3580",
                "globalSlot": "60700",
                "startTime": "1792625400000",
                "endTime": "1792625490000",
            }],
            "globalSlotSinceGenesis": ["920260"],
            "generatedFromConsensusAt": generated,
        } } } })
    );

    let response = execute(node(None), "N", NEXT_BLOCK_PRODUCTION, json!({})).await;
    assert_eq!(
        response,
        json!({ "data": { "daemonStatus": { "nextBlockProduction": {
            "times": [],
            "globalSlotSinceGenesis": [],
            "generatedFromConsensusAt": generated,
        } } } })
    );
}

/// `stakingActive` is OCaml's `is_actively_staking` (`types.ml`
/// L1295-L1315, L1556-L1627): whether the account's key is one of the
/// node's block production keys, for MINA accounts only, and true for every
/// account of a delegator list.
#[tokio::test]
async fn staking_active_as_ocaml() {
    let producer = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
    let delegator = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let key = |k: &str| mina_signer::CompressedPubKey::from_address(k).unwrap();
    let mut producer_account = Account::create_with(
        AccountId::new_with_default_token(key(producer)),
        Balance::from_u64(1),
    );
    producer_account.delegate = Some(key(producer));
    let mut delegator_account = Account::create_with(
        AccountId::new_with_default_token(key(delegator)),
        Balance::from_u64(1),
    );
    delegator_account.delegate = Some(key(producer));
    let token_account = Account::create_with(
        AccountId::new(key(producer), ledger::TokenId::from(7u64)),
        Balance::from_u64(1),
    );
    let token =
        mina_p2p_messages::v2::TokenIdKeyHash::from(ledger::TokenId::from(7u64)).to_string();
    let mut node = best_chain_node();
    node.accounts = vec![
        producer_account.clone(),
        delegator_account.clone(),
        token_account,
    ];
    node.staking_accounts = vec![producer_account.clone(), delegator_account.clone()];
    node.last_epoch_delegators = Some(vec![delegator_account]);
    let query = format!(
        "query S {{ p: account(publicKey: \"{producer}\") {{ stakingActive \
         lastEpochDelegators {{ publicKey stakingActive }} }} \
         d: account(publicKey: \"{delegator}\") {{ stakingActive }} \
         t: account(publicKey: \"{producer}\", token: \"{token}\") {{ stakingActive }} }}"
    );

    // A node without a block producer.
    let response = execute(node.clone(), "S", &query, json!({})).await;
    assert_eq!(
        response,
        json!({ "data": {
            "p": { "stakingActive": false, "lastEpochDelegators": [{ "publicKey": delegator, "stakingActive": true }] },
            "d": { "stakingActive": false },
            "t": { "stakingActive": false },
        } })
    );

    node.status = Some(producer_status(Some(producer), None));
    let response = execute(node, "S", &query, json!({})).await;
    assert_eq!(
        response,
        json!({ "data": {
            "p": { "stakingActive": true, "lastEpochDelegators": [{ "publicKey": delegator, "stakingActive": true }] },
            "d": { "stakingActive": false },
            "t": { "stakingActive": false },
        } })
    );
}

/// The producer and epoch-stake fields of four devnet accounts, as the public
/// OCaml 4.0.2 devnet node (no block producer) answered them
/// (`epoch-staking-devnet.json`): a self-delegated producer key, an account
/// delegating to another key, one created after the staking ledger's
/// snapshot (so not in it: `epochDelegateAccount` null) and a timed one. The
/// fake node holds each account's best tip and staking ledger delegate, as
/// our devnet node had them at the time; the OCaml node's root has crossed an
/// epoch since it started, so its `lastEpochDelegators` is `[]` for every
/// key, and null on a node whose root has not (`EpochDelegatees`).
#[tokio::test]
async fn epoch_staking_fields_as_ocaml_devnet() {
    let fixture = ocaml_response("epoch-staking-devnet");
    let ocaml = &fixture["response"];
    let query = fixture["query"].as_str().unwrap();
    let key = |k: &Value| mina_signer::CompressedPubKey::from_address(k.as_str().unwrap()).unwrap();
    let mut node = best_chain_node();
    for name in [
        "selfDelegated",
        "delegatesOther",
        "notInStakingLedger",
        "timed",
    ] {
        let account = &ocaml["data"][name];
        let id = AccountId::new_with_default_token(key(&account["publicKey"]));
        let mut best = Account::create_with(id.clone(), Balance::from_u64(1));
        best.delegate = Some(key(&account["delegate"]));
        node.accounts.push(best);
        if let Some(delegate) = account["epochDelegateAccount"].get("publicKey") {
            let mut staking = Account::create_with(id, Balance::from_u64(1));
            staking.delegate = Some(key(delegate));
            node.staking_accounts.push(staking);
        }
    }

    node.last_epoch_delegators = Some(vec![]);
    let response = execute(node.clone(), "EpochStaking", query, json!({})).await;
    assert_json_eq(&response, ocaml, "$");

    // Before the root has crossed an epoch.
    node.last_epoch_delegators = None;
    let response = execute(node, "EpochStaking", query, json!({})).await;
    let mut before = ocaml.clone();
    for name in [
        "selfDelegated",
        "delegatesOther",
        "notInStakingLedger",
        "timed",
    ] {
        before["data"][name]["lastEpochDelegators"] = Value::Null;
    }
    assert_json_eq(&response, &before, "$");
}

/// `--public-graphql` takes the account's staking fields, which cost a
/// staking ledger lookup or the node status and scan nothing, and refuses
/// `daemonStatus.nextBlockProduction`, which is about the operator's block
/// producer. Whole-ledger `accounts` remains private, as R34 requires.
#[tokio::test]
async fn public_policy_takes_staking_fields() {
    let key = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
    let query = format!(
        "{{ account(publicKey: \"{key}\") {{ stakingActive \
         epochDelegateAccount {{ publicKey }} lastEpochDelegators {{ publicKey }} }} }}"
    );
    assert!(public(json!({ "query": query })).await.is_ok());
    let accounts = format!(
        "{{ accounts(publicKey: \"{key}\") {{ stakingActive epochDelegateAccount {{ publicKey }} }} }}"
    );
    assert_eq!(
        public(json!({ "query": accounts })).await,
        Err("PUBLIC_GRAPHQL_NOT_ALLOWED".to_owned())
    );
    assert_eq!(
        public(json!({ "query": "{ daemonStatus { nextBlockProduction { globalSlotSinceGenesis } } }" }))
            .await,
        Err("PUBLIC_GRAPHQL_NOT_ALLOWED".to_owned())
    );
}

/// More than eight real delegators must not multiply nested staking lookups
/// beyond the public cost factor; private mode preserves the full OCaml list.
#[tokio::test]
async fn last_epoch_delegators_bounds_nested_work_in_both_modes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let producer = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
    let key = mina_signer::CompressedPubKey::from_address(producer).unwrap();
    let owner = Account::create_with(
        AccountId::new_with_default_token(key.clone()),
        Balance::from_u64(1),
    );
    let entries: Vec<_> = (0..14)
        .map(|_| {
            let mut account = Account::create_with(
                AccountId::new_with_default_token(ledger::gen_compressed()),
                Balance::from_u64(1),
            );
            account.delegate = Some(key.clone());
            account
        })
        .collect();
    let query = format!("{{ account(publicKey: \"{producer}\") {{ lastEpochDelegators {{ publicKey epochDelegateAccount {{ publicKey }} }} }} }}");
    for public in [false, true] {
        let count = Arc::new(AtomicUsize::new(0));
        let mut node = best_chain_node();
        node.accounts = vec![owner.clone()];
        node.staking_accounts = entries.clone();
        // Deliberately return an oversized RPC result too: the resolver's
        // bound protects nested work even from a changed ledger service.
        node.last_epoch_delegators = Some(entries.clone());
        node.expected_last_epoch_limit = Some(public.then_some(8));
        node.staking_lookups = Some(count.clone());
        let (status, response) = respond_http(node.spawn(), json!({"query": query}), public).await;
        assert_eq!(status, 200, "{response}");
        assert!(response.get("errors").is_none(), "{response}");
        let list = response["data"]["account"]["lastEpochDelegators"]
            .as_array()
            .unwrap();
        let expected = if public { 8 } else { 14 };
        assert_eq!(list.len(), expected);
        assert_eq!(count.load(Ordering::SeqCst), expected);
        for (row, account) in list.iter().zip(&entries) {
            assert_eq!(row["publicKey"], account.public_key.into_address());
            assert_eq!(row["epochDelegateAccount"]["publicKey"], producer);
        }
    }
    // Aliases and recursive selections still pass through R34's walker.
    let aliases = (0..5)
        .map(|i| {
            format!("a{i}: lastEpochDelegators {{ epochDelegateAccount {{ stakingActive }} }}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let query = format!("{{ account(publicKey: \"{producer}\") {{ {aliases} }} }}");
    assert_eq!(
        public(json!({"query": query})).await,
        Err("PUBLIC_GRAPHQL_TOO_COMPLEX".to_owned())
    );
}

/// LN2: a ledger of a few accounts with `key`'s at index 2, as the node
/// holds it, and the fake node answering from it.
fn light_node(key: &str) -> (FakeNode, ledger::Mask) {
    use ledger::BaseLedger;
    let mut mask = ledger::Mask::create(35);
    let mut accounts = Vec::new();
    for n in 0..5u64 {
        let account = if n == 2 {
            Account::create_with(
                AccountId::new_with_default_token(
                    mina_signer::CompressedPubKey::from_address(key).unwrap(),
                ),
                Balance::from_u64(42_000_000_000),
            )
        } else {
            Account::rand()
        };
        mask.get_or_create_account(account.id(), account.clone())
            .unwrap();
        accounts.push(account);
    }
    let node = FakeNode {
        accounts,
        light_ledger: Some(mask.clone()),
        ..best_chain_node()
    };
    (node, mask)
}

/// OCaml's `Merkle_path.implied_root` over a GraphQL `merklePath`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/merkle_path.ml#L32-L42>
fn graphql_implied_root(leaf: &str, path: &Value) -> (mina_p2p_messages::v2::LedgerHash, u64) {
    use ledger::TreeVersion;
    use mina_p2p_messages::bigint::BigInt;
    let field = |text: &str| BigInt::from_decimal(text).unwrap().to_field().unwrap();
    let mut index = 0;
    let root =
        path.as_array()
            .unwrap()
            .iter()
            .enumerate()
            .fold(field(leaf), |node, (height, element)| {
                match (element["left"].as_str(), element["right"].as_str()) {
                    (Some(right), None) => {
                        <ledger::V2 as TreeVersion>::hash_node(height, node, field(right))
                    }
                    (None, Some(left)) => {
                        index |= 1 << height;
                        <ledger::V2 as TreeVersion>::hash_node(height, field(left), node)
                    }
                    _ => panic!("bad element {element}"),
                }
            });
    (mina_p2p_messages::v2::LedgerHash::from_fp(root), index)
}

#[tokio::test]
async fn account_index_leaf_hash_and_merkle_path() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    use ledger::BaseLedger;
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let (node, mut mask) = light_node(key);
    let lookups = node.light_lookups.clone();
    let expected_leaf = node.accounts[2].hash();
    let query = format!(
        "query Path {{ account(publicKey: \"{key}\") {{ index leafHash merklePath {{ left right }} }} }}"
    );
    let response = execute(node, "Path", &query, json!({})).await;
    assert_eq!(response.get("errors"), None, "{response}");
    let account = &response["data"]["account"];
    assert_eq!(account["index"], 2);
    let leaf = account["leafHash"].as_str().unwrap();
    assert_eq!(
        leaf,
        mina_p2p_messages::bigint::BigInt::from(&expected_leaf).to_decimal()
    );
    assert_eq!(account["merklePath"].as_array().unwrap().len(), 35);
    let (root, index) = graphql_implied_root(leaf, &account["merklePath"]);
    assert_eq!(index, 2);
    assert_eq!(
        root,
        mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root())
    );
    // `index` and `merklePath` share one ledger read.
    assert_eq!(lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// While the node is not synced, OCaml has no best tip ledger: null.
#[tokio::test]
async fn merkle_path_is_null_while_not_synced() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let (mut node, _) = light_node(key);
    node.light_ledger = None;
    let query = format!(
        "query Path {{ account(publicKey: \"{key}\") {{ index leafHash merklePath {{ left right }} }} }}"
    );
    let response = execute(node, "Path", &query, json!({})).await;
    assert_eq!(response.get("errors"), None, "{response}");
    let account = &response["data"]["account"];
    assert_eq!(account["index"], Value::Null);
    assert_eq!(account["merklePath"], Value::Null);
    // The leaf comes from the same read as the path.
    assert_eq!(account["leafHash"], Value::Null);
}

#[tokio::test]
async fn encoded_snarked_ledger_account_membership() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use ledger::BaseLedger;
    use mina_p2p_messages::{binprot::BinProtRead, v2::MinaBaseAccountBinableArgStableV2};
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let state_hash = "3NLeXS7Kxv68v6hMoS6VZ28yubTEn2G551BKQ6YYxjFG1aYz21S4";
    let (mut node, mut mask) = light_node(key);
    node.light_block = Some(state_hash.parse().unwrap());
    let query = |key: &str, hash: &str| {
        format!(
            "query M {{ encodedSnarkedLedgerAccountMembership(accountInfos: [{{publicKey: \"{key}\"}}], stateHash: \"{hash}\") {{ account merklePath {{ left right }} }} }}"
        )
    };
    let response = execute(node.clone(), "M", &query(key, state_hash), json!({})).await;
    assert_eq!(response.get("errors"), None, "{response}");
    let membership = &response["data"]["encodedSnarkedLedgerAccountMembership"][0];
    let bytes = STANDARD
        .decode(membership["account"].as_str().unwrap())
        .unwrap();
    let wire = MinaBaseAccountBinableArgStableV2::binprot_read(&mut &bytes[..]).unwrap();
    let account = Account::try_from(&wire).unwrap();
    assert_eq!(account.public_key.into_address(), key);
    let leaf = mina_p2p_messages::bigint::BigInt::from(&account.hash()).to_decimal();
    let (root, index) = graphql_implied_root(&leaf, &membership["merklePath"]);
    assert_eq!(index, 2);
    assert_eq!(
        root,
        mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root())
    );

    // OCaml's failures, as GraphQL errors.
    let missing = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
    let other_block = "3NL7hPYvyBXoFaqKJPtAfPqKPgRP3oRjwFNbmaxpdt4Qna6YuH2i";
    for (key, hash, message) in [
        (missing, state_hash, "Account not found in snarked ledger"),
        (key, other_block, "Failed to get snarked ledger: "),
        (key, "3Nnot-a-hash", "Invalid state hash"),
    ] {
        let response = execute(node.clone(), "M", &query(key, hash), json!({})).await;
        let error = response["errors"][0]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(error.starts_with(message), "{key} {hash}: {response}");
    }
}

/// A block changes the account between the read of the account object and
/// the read of its path. `leafHash`, `index` and `merklePath` all come from
/// the later read, one snapshot, so the path proves the leaf; the earlier
/// code hashed the account object, so the path proved nothing.
#[tokio::test]
async fn path_fields_come_from_one_snapshot_when_a_block_lands_between_reads() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    use ledger::BaseLedger;
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let (mut node, mut mask) = light_node(key);
    // The account object the resolver loads first: as before the block.
    let old = node.accounts[2].clone();
    // The ledger the path is read from: after a block paid the account.
    let address = mask.location_of_account(&old.id()).unwrap();
    let mut new = (*mask.get(address.clone()).unwrap()).clone();
    new.balance = Balance::from_u64(43_000_000_000);
    mask.set(address, Box::new(new.clone()));
    node.light_ledger = Some(mask.clone());
    assert_ne!(old.hash(), new.hash());

    let query = format!(
        "query Path {{ account(publicKey: \"{key}\") {{ balance {{ total }} index leafHash merklePath {{ left right }} }} }}"
    );
    let response = execute(node, "Path", &query, json!({})).await;
    assert_eq!(response.get("errors"), None, "{response}");
    let account = &response["data"]["account"];
    // The object's own fields are from the first read.
    assert_eq!(account["balance"]["total"], "42000000000");
    let leaf = account["leafHash"].as_str().unwrap();
    assert_eq!(
        leaf,
        mina_p2p_messages::bigint::BigInt::from(&new.hash()).to_decimal(),
        "leafHash hashes the account the path was read with"
    );
    let (root, index) = graphql_implied_root(leaf, &account["merklePath"]);
    assert_eq!(index, account["index"].as_u64().unwrap());
    assert_eq!(
        root,
        mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root())
    );
    // The object's hash, as the earlier code served it, proves nothing.
    let (stale, _) = graphql_implied_root(
        &mina_p2p_messages::bigint::BigInt::from(&old.hash()).to_decimal(),
        &account["merklePath"],
    );
    assert_ne!(stale, root);
}

/// The path fields of many accounts in one query are one ledger call per 32
/// accounts, not one per account.
#[tokio::test]
async fn many_accounts_paths_are_batched() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    use ledger::BaseLedger;
    let (mut node, mut mask) =
        light_node("B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U");
    for _ in 0..35 {
        // MINA accounts, which `account(publicKey)` finds.
        let mut account = Account::rand();
        account.token_id = ledger::TokenId::default();
        mask.get_or_create_account(account.id(), account.clone())
            .unwrap();
        node.accounts.push(account);
    }
    node.light_ledger = Some(mask.clone());
    let lookups = node.light_lookups.clone();
    let mina: Vec<Account> = node
        .accounts
        .iter()
        .filter(|account| account.token_id.is_default())
        .cloned()
        .collect();
    assert!(mina.len() > 32);
    let fields: String = mina
        .iter()
        .enumerate()
        .map(|(n, account)| {
            format!(
                "a{n}: account(publicKey: \"{}\") {{ leafHash merklePath {{ left right }} }} ",
                account.public_key.into_address()
            )
        })
        .collect();
    let response = execute(
        node.clone(),
        "Many",
        &format!("query Many {{ {fields} }}"),
        json!({}),
    )
    .await;
    assert_eq!(response.get("errors"), None, "{response}");
    let root = mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root());
    for n in 0..mina.len() {
        let account = &response["data"][format!("a{n}")];
        let (folded, _) = graphql_implied_root(
            account["leafHash"]
                .as_str()
                .unwrap_or_else(|| panic!("a{n}: {account}")),
            &account["merklePath"],
        );
        assert_eq!(folded, root, "a{n}");
    }
    // 36 to 40 accounts: two ledger calls of at most 32.
    assert_eq!(
        lookups.load(std::sync::atomic::Ordering::SeqCst),
        mina.len().div_ceil(32)
    );
}

#[tokio::test]
async fn membership_takes_at_most_32_accounts_in_one_ledger_call() {
    // The light permits are shared with the HTTP light tests.
    let _guard = crate::http_server::routes::ledger::EXPORT_TEST_LOCK
        .lock()
        .await;
    let key = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
    let state_hash = "3NLeXS7Kxv68v6hMoS6VZ28yubTEn2G551BKQ6YYxjFG1aYz21S4";
    let (mut node, _) = light_node(key);
    node.light_block = Some(state_hash.parse().unwrap());
    let query = |count: usize| {
        let infos = vec![format!("{{publicKey: \"{key}\"}}"); count].join(", ");
        format!(
            "query M {{ encodedSnarkedLedgerAccountMembership(accountInfos: [{infos}], stateHash: \"{state_hash}\") {{ account }} }}"
        )
    };
    assert_eq!(super::membership::MAX_MEMBERSHIP_ACCOUNTS, 32);
    let lookups = node.light_lookups.clone();
    let response = execute(node.clone(), "M", &query(32), json!({})).await;
    assert_eq!(response.get("errors"), None, "{response}");
    assert_eq!(
        response["data"]["encodedSnarkedLedgerAccountMembership"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
    let response = execute(node, "M", &query(33), json!({})).await;
    let error = response["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(error.contains("at most 32"), "{response}");
    // Refused before any ledger read.
    assert_eq!(lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
}
