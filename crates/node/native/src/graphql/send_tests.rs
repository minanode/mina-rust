//! P6i: `sendPayment` and `sendDelegation` as OCaml 4.0.2 answers them: the
//! fee and memo checks, "Daemon is bootstrapping", nonce inference from the
//! pool and the ledger, the explicit nonce range, and the signature check for
//! this node's network, all before anything reaches the pool.

use std::{
    any::Any,
    sync::{Arc, Mutex},
};

use juniper::{http::GraphQLRequest, EmptySubscription, RootNode};
use ledger::{
    scan_state::{
        currency::{Amount, Balance, Fee, Nonce},
        transaction_logic::{
            signed_command::{
                Body, PaymentPayload, SignedCommand, SignedCommandPayload, StakeDelegationPayload,
            },
            transaction_union_payload::TransactionUnionPayload,
            valid, Memo,
        },
    },
    Account, AccountId, TokenId,
};
use mina_core::network::NetworkConfig;
use mina_node::{
    account::AccountSecretKey,
    core::channels::{mpsc, oneshot},
    rpc::{
        AccountQuery, RpcLedgerAccountsResponse, RpcRequest, RpcTransactionInjectResponse,
        RpcTransactionNonceGetResponse,
    },
};
use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
use mina_p2p_messages::{bigint::BigInt, v2::MinaBaseUserCommandStableV2};
use mina_signer::{Keypair, NetworkId, Signer};
use serde_json::{json, Value};

use super::{Context, Mutation, Query};

type Schema = RootNode<Query, Mutation, EmptySubscription<Context>>;

/// Answers the requests the send resolvers make. `injected` records what
/// reached the pool.
#[derive(Clone, Default)]
struct FakeNode {
    bootstrapping: bool,
    accounts: Vec<Account>,
    pool_nonce: Option<u32>,
    /// The sender's ledger nonce, or `None` when it has no account.
    ledger_nonce: Option<u32>,
    injected: Arc<Mutex<Vec<MinaBaseUserCommandStableV2>>>,
}

fn respond<T: 'static + Send>(responder: Box<dyn Send + Any>, response: T) {
    let sender = responder
        .downcast::<oneshot::Sender<T>>()
        .expect("the resolver expects another response type");
    let _ = sender.send(response);
}

fn sender_key() -> AccountSecretKey {
    AccountSecretKey::deterministic(0)
}

fn receiver_key() -> AccountSecretKey {
    AccountSecretKey::deterministic(1)
}

impl FakeNode {
    fn handle(&self, NodeRpcRequest { req, responder }: NodeRpcRequest) {
        match req {
            RpcRequest::TransactionNonceGet(_) => respond(
                responder,
                RpcTransactionNonceGetResponse {
                    bootstrapping: self.bootstrapping,
                    pool_nonce: self.pool_nonce,
                },
            ),
            RpcRequest::LedgerAccountsGet(AccountQuery::MultipleIds(ids)) => {
                let mut accounts = self.accounts.clone();
                if let Some(nonce) = self.ledger_nonce {
                    let mut sender = Account::create_with(
                        AccountId::new_with_default_token(sender_key().public_key_compressed()),
                        Balance::from_u64(10_000_000_000),
                    );
                    sender.nonce = Nonce::from_u32(nonce);
                    accounts.push(sender);
                }
                accounts.retain(|account| ids.contains(&account.id()));
                respond::<RpcLedgerAccountsResponse>(responder, accounts);
            }
            RpcRequest::LedgerAccountsGet(AccountQuery::PubKeyWithTokenId(key, _)) => {
                let key: mina_signer::CompressedPubKey = key.try_into().unwrap();
                let response: RpcLedgerAccountsResponse = match self.ledger_nonce {
                    Some(nonce) if key == sender_key().public_key_compressed() => {
                        let mut account = Account::create_with(
                            AccountId::new(key, TokenId::default()),
                            Balance::from_u64(10_000_000_000),
                        );
                        account.nonce = Nonce::from_u32(nonce);
                        vec![account]
                    }
                    _ => self
                        .accounts
                        .iter()
                        .filter(|a| a.public_key == key)
                        .cloned()
                        .collect(),
                };
                respond(responder, response)
            }
            RpcRequest::TransactionInject(commands) => {
                self.injected.lock().unwrap().extend(commands.clone());
                let valids = commands
                    .iter()
                    .map(|c| valid::UserCommand::try_from(c).unwrap())
                    .collect();
                respond(responder, RpcTransactionInjectResponse::Success(valids))
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

async fn execute(node: &FakeNode, query: &str, variables: Value) -> Value {
    execute_with_submission(node, query, variables, true).await
}

async fn execute_with_submission(
    node: &FakeNode,
    query: &str,
    variables: Value,
    enabled: bool,
) -> Value {
    let _ = NetworkConfig::init("mainnet");
    let request: GraphQLRequest = serde_json::from_value(json!({
        "query": query,
        "variables": variables,
    }))
    .unwrap();
    let schema = Schema::new(Query, Mutation, EmptySubscription::new());
    let context = Context::new(node.clone().spawn()).with_tx_submission(enabled);
    let response = request.execute(&schema, &context).await;
    serde_json::to_value(&response).unwrap()
}

const SEND_PAYMENT: &str = r#"
mutation ($input: SendPaymentInput!, $signature: SignatureInput!) {
  sendPayment(input: $input, signature: $signature) {
    payment { nonce memo }
  }
}
"#;

const SEND_DELEGATION: &str = r#"
mutation ($input: SendDelegationInput!, $signature: SignatureInput!) {
  sendDelegation(input: $input, signature: $signature) {
    delegation { nonce }
  }
}
"#;

const FEE: u64 = 10_000_000;
const AMOUNT: u64 = 2_000_000_000;

/// Signs a payment or a delegation the way a wallet does for `network`, with
/// OCaml's legacy nonce derivation.
fn sign(nonce: u32, body: Body, network: NetworkId) -> (String, String) {
    let _ = NetworkConfig::init("mainnet");
    let from = sender_key().public_key_compressed();
    let payload = SignedCommandPayload::create(
        Fee::from_u64(FEE),
        from,
        Nonce::from_u32(nonce),
        None,
        Memo::empty(),
        body,
    );
    let mut signer = mina_signer::create_legacy::<TransactionUnionPayload>(network);
    let keypair: Keypair = sender_key().into();
    let signature = signer.sign(
        &keypair,
        &TransactionUnionPayload::of_user_command_payload(&payload),
        mina_signer::NonceMode::Legacy,
    );
    (
        BigInt::from(signature.rx).to_decimal(),
        BigInt::from(signature.s).to_decimal(),
    )
}

fn payment_body() -> Body {
    Body::Payment(PaymentPayload {
        receiver_pk: receiver_key().public_key_compressed(),
        amount: Amount::from_u64(AMOUNT),
    })
}

fn delegation_body() -> Body {
    Body::StakeDelegation(StakeDelegationPayload::SetDelegate {
        new_delegate: receiver_key().public_key_compressed(),
    })
}

fn payment_variables(nonce: Option<u32>, signed_nonce: u32, network: NetworkId) -> Value {
    let (field, scalar) = sign(signed_nonce, payment_body(), network);
    let mut input = json!({
        "from": sender_key().public_key().to_string(),
        "to": receiver_key().public_key().to_string(),
        "amount": AMOUNT.to_string(),
        "fee": FEE.to_string(),
    });
    if let Some(nonce) = nonce {
        input["nonce"] = json!(nonce.to_string());
    }
    json!({ "input": input, "signature": { "field": field, "scalar": scalar } })
}

fn errors(response: &Value) -> Vec<String> {
    response["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|e| e["message"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn injected_nonces(node: &FakeNode) -> Vec<u32> {
    node.injected
        .lock()
        .unwrap()
        .iter()
        .map(|cmd| match cmd {
            MinaBaseUserCommandStableV2::SignedCommand(c) => c.payload.common.nonce.as_u32(),
            _ => unreachable!(),
        })
        .collect()
}

/// The injected command's signature must verify: the bytes our resolver
/// built are the bytes the wallet signed.
fn injected_signature_is_valid(node: &FakeNode) -> bool {
    node.injected.lock().unwrap().iter().all(|cmd| {
        let MinaBaseUserCommandStableV2::SignedCommand(c) = cmd else {
            return false;
        };
        let c = SignedCommand::try_from(c).unwrap();
        super::user_command::check_signature(&c).is_ok()
    })
}

#[tokio::test]
async fn a_fresh_account_sends_with_the_nonce_omitted() {
    // OCaml infers the ledger nonce, 0; we used to send 1 (A2).
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 0, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(response["data"]["sendPayment"]["payment"]["nonce"], 0);
    assert_eq!(injected_nonces(&node), vec![0]);
    assert!(injected_signature_is_valid(&node));
}

#[tokio::test]
async fn a_fresh_account_sends_with_an_explicit_nonce_of_0() {
    // We used to refuse an explicit 0 (A3).
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(Some(0), 0, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(injected_nonces(&node), vec![0]);
}

#[tokio::test]
async fn the_nonce_after_pooled_commands_is_inferred() {
    let node = FakeNode {
        ledger_nonce: Some(1),
        pool_nonce: Some(3),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 3, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(injected_nonces(&node), vec![3]);
    assert!(injected_signature_is_valid(&node));

    // A lower explicit nonce replaces a pooled command: it is allowed down to
    // the ledger nonce.
    let node = FakeNode {
        ledger_nonce: Some(1),
        pool_nonce: Some(3),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(Some(1), 1, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(injected_nonces(&node), vec![1]);
}

#[tokio::test]
async fn a_nonce_out_of_range_is_refused_as_ocaml_words_it() {
    let node = FakeNode {
        ledger_nonce: Some(1),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(Some(5), 5, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(
        errors(&response),
        vec![
            "Couldn't send user command: Error creating user command: Error: Input nonce 5 \
              either different from inferred nonce 1 or below minimum_nonce 1"
                .to_owned()
        ]
    );
    assert!(injected_nonces(&node).is_empty());
}

#[tokio::test]
async fn an_unknown_sender_is_refused() {
    let node = FakeNode::default();
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 0, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(
        errors(&response),
        vec![super::user_command::creation_error(
            super::user_command::CANNOT_INFER_NONCE
        )]
    );
    assert!(injected_nonces(&node).is_empty());
}

/// A signature for another network (a devnet wallet on a mainnet node) used
/// to hang the request: the pool's verifier failure was never answered.
#[tokio::test]
async fn a_wrong_network_signature_is_refused() {
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 0, NetworkId::TESTNET),
    )
    .await;
    assert_eq!(
        errors(&response),
        vec![
            "Couldn't send user command: Error creating user command: Error: Invalid_signature"
                .to_owned()
        ]
    );
    assert!(injected_nonces(&node).is_empty());
}

#[tokio::test]
async fn while_bootstrapping_the_daemon_says_so() {
    let node = FakeNode {
        bootstrapping: true,
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 0, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(
        errors(&response),
        vec!["Daemon is bootstrapping".to_owned()]
    );
    assert!(injected_nonces(&node).is_empty());
}

#[tokio::test]
async fn a_long_memo_and_a_low_fee_are_refused_first() {
    let node = FakeNode {
        bootstrapping: true,
        ..Default::default()
    };
    let mut variables = payment_variables(None, 0, NetworkId::MAINNET);
    variables["input"]["memo"] = json!("x".repeat(33));
    let response = execute(&node, SEND_PAYMENT, variables).await;
    assert_eq!(
        errors(&response),
        vec!["Invalid `memo` provided.".to_owned()]
    );

    let mut variables = payment_variables(None, 0, NetworkId::MAINNET);
    variables["input"]["fee"] = json!("100000");
    let response = execute(&node, SEND_PAYMENT, variables).await;
    assert_eq!(
        errors(&response),
        vec!["Invalid user command. Fee 0.0001 is less than the minimum fee, 0.001.".to_owned()]
    );
}

#[tokio::test]
async fn a_delegation_from_a_fresh_account_is_sent() {
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let (field, scalar) = sign(0, delegation_body(), NetworkId::MAINNET);
    let variables = json!({
        "input": {
            "from": sender_key().public_key().to_string(),
            "to": receiver_key().public_key().to_string(),
            "fee": FEE.to_string(),
        },
        "signature": { "field": field, "scalar": scalar },
    });
    let response = execute(&node, SEND_DELEGATION, variables).await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(response["data"]["sendDelegation"]["delegation"]["nonce"], 0);
    assert!(injected_signature_is_valid(&node));
}

/// Pubsub data as our node publishes it (`encode_message` in the p2p crate):
/// an 8-byte little-endian length, then the bin_prot message.
fn pubsub_data(message: &mina_p2p_messages::gossip::GossipNetMessageV2) -> Vec<u8> {
    use mina_p2p_messages::binprot::BinProtWrite;
    let mut data = vec![0; 8];
    message.binprot_write(&mut data).unwrap();
    let len = (data.len() - 8) as u64;
    data[..8].copy_from_slice(&len.to_le_bytes());
    data
}

fn tx_diff(
    commands: Vec<MinaBaseUserCommandStableV2>,
    nonce: u64,
) -> mina_p2p_messages::gossip::GossipNetMessageV2 {
    mina_p2p_messages::gossip::GossipNetMessageV2::TransactionPoolDiff {
        message: mina_p2p_messages::v2::NetworkPoolTransactionPoolDiffVersionedStableV2(
            commands.into_iter().collect(),
        ),
        nonce: nonce.into(),
    }
}

/// Level 2's messages, for `tools/tx-gossip-probe`: run with
/// `P6_PROBE_OUT=<dir>` and `--ignored`. `payment.bin` is a payment from the
/// deterministic test key 0 to key 1, built by `sendPayment` exactly as for a
/// submission and then taken through the pool's conversions, as our node
/// rebroadcasts it. `padded.bin` is the same diff wrapped as P6a refuses: a
/// size header covering four junk bytes.
#[tokio::test]
#[ignore = "writes Level 2 probe messages to $P6_PROBE_OUT"]
async fn write_level2_probe_messages() {
    let out = std::path::PathBuf::from(std::env::var("P6_PROBE_OUT").expect("P6_PROBE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let nonce = std::env::var("P6_PROBE_NONCE")
        .map(|n| n.parse::<u32>().unwrap())
        .unwrap_or(0);

    let node = FakeNode {
        ledger_nonce: Some(nonce),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, nonce, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    let submitted = node.injected.lock().unwrap()[0].clone();

    // The pool keeps ledger types; `Rebroadcast` converts back.
    let pooled = ledger::scan_state::transaction_logic::UserCommand::try_from(&submitted).unwrap();
    let rebroadcast: MinaBaseUserCommandStableV2 =
        (&pooled.to_valid_unsafe().forget_check()).into();
    assert_eq!(rebroadcast, submitted, "the pool's round trip is lossless");

    let payment = pubsub_data(&tx_diff(vec![rebroadcast], 0));
    std::fs::write(out.join("payment.bin"), &payment).unwrap();

    let mut padded = payment[8..].to_vec();
    padded.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let mut data = (padded.len() as u64).to_le_bytes().to_vec();
    data.extend_from_slice(&padded);
    std::fs::write(out.join("padded.bin"), data).unwrap();

    println!(
        "sender {} receiver {}",
        sender_key().public_key(),
        receiver_key().public_key()
    );
}

const OCAML_ID: &str = "Av2AlpgAc/XQUwSMGPSk0JzHIX5JUKoEZGyf3bR2M5aU4c4U6jwAAAD//yIBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFgZ7h6IQr5YpavcUprGDohsPWMf02aoFt5b7a1IePE3Af0AlDV3c/XQUwSMGPSk0JzHIX5JUKoEZGyf3bR2M5aU4c4U6jwAq4MQ8MzRo0tXRLOVUMjH0MIcR6JlAiHtz/NDSbM0nRGu75ZN2s+u/9vXAmD9LqjDL+k+X1XHyS7dKrUaBdeDNg==";

/// The payment of Level 2, as OCaml 4.0.2 showed it in `pooledUserCommands`
/// after taking it from our probe into its pool (2026-10-21, isolated daemon
/// on a private genesis, private implementation notes).
#[tokio::test]
async fn the_level2_payment_hashes_as_ocaml_did() {
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let response = execute(
        &node,
        SEND_PAYMENT,
        payment_variables(None, 0, NetworkId::MAINNET),
    )
    .await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    let injected = node.injected.lock().unwrap()[0].clone();
    let MinaBaseUserCommandStableV2::SignedCommand(signed) = &injected else {
        unreachable!()
    };
    // OCaml's `id`: the command's bytes, so equal ids mean OCaml stored the
    // bytes we built.
    assert_eq!(signed.to_base64().unwrap(), OCAML_ID);
    assert_eq!(
        injected.hash().unwrap().to_string(),
        "5JuE382hBLMggPEdHx665FkrzNJL6Vcnbqpq7S6iZzoxrUwabbFb"
    );
}

/// A payment of `amount` with the nonce omitted, signed for mainnet.
fn signed_payment(amount: u64) -> (String, String) {
    sign(
        0,
        Body::Payment(PaymentPayload {
            receiver_pk: receiver_key().public_key_compressed(),
            amount: Amount::from_u64(amount),
        }),
        NetworkId::MAINNET,
    )
}

fn fresh_node() -> FakeNode {
    FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    }
}

/// UInt64 and UInt32 take strings and integers, as OCaml's
/// `make_numeric_arg` does: JSON integers in variables, including amounts
/// beyond 2^31.
#[tokio::test]
async fn numeric_variables_take_integers_as_ocaml_does() {
    for amount in [AMOUNT, 3_000_000_000] {
        let node = fresh_node();
        let (field, scalar) = signed_payment(amount);
        let variables = json!({
            "input": {
                "from": sender_key().public_key().to_string(),
                "to": receiver_key().public_key().to_string(),
                "amount": amount,
                "fee": FEE,
                "nonce": 0,
            },
            "signature": { "field": field, "scalar": scalar },
        });
        let response = execute(&node, SEND_PAYMENT, variables).await;
        assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
        assert_eq!(injected_nonces(&node), vec![0]);
        assert!(injected_signature_is_valid(&node));
    }

    let node = fresh_node();
    let (field, scalar) = sign(0, delegation_body(), NetworkId::MAINNET);
    let variables = json!({
        "input": {
            "from": sender_key().public_key().to_string(),
            "to": receiver_key().public_key().to_string(),
            "fee": FEE,
            "nonce": 0,
        },
        "signature": { "field": field, "scalar": scalar },
    });
    let response = execute(&node, SEND_DELEGATION, variables).await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert!(injected_signature_is_valid(&node));
}

/// What OCaml refuses: non-canonical decimal strings, values out of range,
/// negative numbers, floats. Nothing reaches the node.
#[tokio::test]
async fn numeric_variables_refuse_what_ocaml_refuses() {
    let (field, scalar) = signed_payment(AMOUNT);
    for (key, value) in [
        ("fee", json!("010000000")),
        ("fee", json!("+10000000")),
        ("fee", json!(" 10000000")),
        ("fee", json!("10_000_000")),
        ("fee", json!("-1")),
        ("fee", json!(-1)),
        ("fee", json!(1.5)),
        ("fee", json!(true)),
        ("amount", json!("18446744073709551616")),
        ("nonce", json!("00")),
        ("nonce", json!("4294967296")),
        ("validUntil", json!("01")),
    ] {
        let node = fresh_node();
        let mut input = json!({
            "from": sender_key().public_key().to_string(),
            "to": receiver_key().public_key().to_string(),
            "amount": AMOUNT.to_string(),
            "fee": FEE.to_string(),
        });
        input[key] = value.clone();
        let variables = json!({
            "input": input,
            "signature": { "field": field, "scalar": scalar },
        });
        let response = execute(&node, SEND_PAYMENT, variables).await;
        assert!(!errors(&response).is_empty(), "{key} = {value}: {response}");
        assert!(injected_nonces(&node).is_empty(), "{key} = {value}");
    }

    // The largest values of each type are taken: u64::MAX as a string for
    // the fee (the request then fails on the signature, past coercion).
    let node = fresh_node();
    let variables = json!({
        "input": {
            "from": sender_key().public_key().to_string(),
            "to": receiver_key().public_key().to_string(),
            "amount": AMOUNT.to_string(),
            "fee": u64::MAX.to_string(),
            "nonce": "0",
            "validUntil": u32::MAX.to_string(),
        },
        "signature": { "field": field, "scalar": scalar },
    });
    let response = execute(&node, SEND_PAYMENT, variables).await;
    assert_eq!(
        errors(&response),
        vec![super::user_command::creation_error("Invalid_signature")],
        "{response}"
    );
}

fn literal_payment(fee: &str, amount: &str, nonce: &str, field: &str, scalar: &str) -> String {
    format!(
        r#"mutation {{
  sendPayment(
    input: {{ from: "{}", to: "{}", amount: {amount}, fee: {fee}, nonce: {nonce} }}
    signature: {{ field: "{field}", scalar: "{scalar}" }}
  ) {{ payment {{ nonce }} }}
}}"#,
        sender_key().public_key(),
        receiver_key().public_key(),
    )
}

/// Literals in the query: integers (above 2^31 too) and strings are taken,
/// floats and non-canonical strings refused, for both mutations.
#[tokio::test]
async fn numeric_literals_as_ocaml() {
    for amount in [AMOUNT, 3_000_000_000] {
        let (field, scalar) = signed_payment(amount);
        for (fee, amount_literal, nonce) in [
            (FEE.to_string(), amount.to_string(), "0".to_owned()),
            (
                format!("\"{FEE}\""),
                format!("\"{amount}\""),
                "\"0\"".to_owned(),
            ),
        ] {
            let node = fresh_node();
            let query = literal_payment(&fee, &amount_literal, &nonce, &field, &scalar);
            let response = execute(&node, &query, json!({})).await;
            assert_eq!(
                errors(&response),
                Vec::<String>::new(),
                "{query}\n{response}"
            );
            assert_eq!(injected_nonces(&node), vec![0]);
        }
    }

    let (field, scalar) = signed_payment(AMOUNT);
    for (fee, nonce) in [
        ("\"010000000\"", "0"),
        ("10000000.0", "0"),
        ("1e7", "0"),
        ("-10000000", "0"),
        (&*FEE.to_string(), "\"00\""),
        (&*FEE.to_string(), "4294967296.5"),
        (&*FEE.to_string(), "0e0"),
    ] {
        let node = fresh_node();
        let query = literal_payment(fee, &AMOUNT.to_string(), nonce, &field, &scalar);
        let response = execute(&node, &query, json!({})).await;
        assert!(!errors(&response).is_empty(), "{query}\n{response}");
        assert!(injected_nonces(&node).is_empty());
    }

    // sendDelegation with integer literals.
    let node = fresh_node();
    let (field, scalar) = sign(0, delegation_body(), NetworkId::MAINNET);
    let query = format!(
        r#"mutation {{
  sendDelegation(
    input: {{ from: "{}", to: "{}", fee: {FEE}, nonce: 0 }}
    signature: {{ field: "{field}", scalar: "{scalar}" }}
  ) {{ delegation {{ nonce }} }}
}}"#,
        sender_key().public_key(),
        receiver_key().public_key(),
    );
    let response = execute(&node, &query, json!({})).await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    for fee in ["\"0010000000\"", "10000000.0", "1e7"] {
        let query = query.replace(&format!("fee: {FEE}"), &format!("fee: {fee}"));
        let node = fresh_node();
        let response = execute(&node, &query, json!({})).await;
        assert!(!errors(&response).is_empty(), "{response}");
        assert!(injected_nonces(&node).is_empty());
    }
}

/// Re-submit a signed payment captured in OCaml's BestChain response to a
/// local fake node. Its id, hash, memo and all SDK-selected values come from
/// OCaml, not from our conversion or signer. No network or funds are used.
#[tokio::test]
async fn payment_payload_matches_ocaml_command() {
    use mina_p2p_messages::v2::MinaBaseSignedCommandStableV2;
    let golden: Value = serde_json::from_str(include_str!(
        "../../tests/files/graphql/ocaml/BestChain.json"
    ))
    .unwrap();
    let mut expected = golden["data"]["bestChain"][0]["transactions"]["userCommands"][0].clone();
    let wire =
        MinaBaseSignedCommandStableV2::from_base64(expected["id"].as_str().unwrap()).unwrap();
    let command = SignedCommand::try_from(&wire).unwrap();
    let mut account = Account::create_with(
        AccountId::new_with_default_token(command.payload.common.fee_payer_pk.clone()),
        Balance::from_u64(u64::MAX),
    );
    account.nonce = command.payload.common.nonce;
    let node = FakeNode {
        accounts: vec![account],
        ..Default::default()
    };
    let variables = json!({
        "input": {
            "from": expected["source"]["publicKey"], "to": expected["receiver"]["publicKey"],
            "amount":expected["amount"], "fee":expected["fee"],
            "nonce": expected["nonce"].as_u64().unwrap().to_string(),
            "memo": command.payload.common.memo.to_string(),
            "validUntil":wire.payload.common.valid_until.as_u32().to_string(),
        },
        "signature": { "field":BigInt::from(command.signature.rx).to_decimal(), "scalar":BigInt::from(command.signature.s).to_decimal() }
    });
    let document = include_str!("../../tests/files/graphql/operations/sdk.graphql");
    let query = document
        .split("mutation SendPayment")
        .nth(1)
        .unwrap()
        .split("mutation SendDelegation")
        .next()
        .unwrap();
    let query = format!("mutation SendPayment{query}");
    expected.as_object_mut().unwrap().remove("failureReason");
    let response = execute(&node, &query, variables.clone()).await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(response["data"]["sendPayment"]["payment"], expected);
    assert!(injected_signature_is_valid(&node));

    let disabled = FakeNode::default();
    let response = execute_with_submission(&disabled, &query, variables.clone(), false).await;
    assert_eq!(
        errors(&response),
        ["transaction submission is disabled on this node"]
    );
    assert!(disabled.injected.lock().unwrap().is_empty());
    let bootstrapping = FakeNode {
        bootstrapping: true,
        ..Default::default()
    };
    let response = execute(&bootstrapping, &query, variables).await;
    assert_eq!(errors(&response), ["Daemon is bootstrapping"]);
    assert!(bootstrapping.injected.lock().unwrap().is_empty());
}

/// Every shared field and both delegation-only fields, including fragment
/// dispatch and OCaml's Partial_account for a receiver absent from the ledger.
#[tokio::test]
async fn delegation_payload_has_ocaml_fields_and_partial_receiver() {
    let node = FakeNode {
        ledger_nonce: Some(0),
        ..Default::default()
    };
    let (field, scalar) = sign(0, delegation_body(), NetworkId::MAINNET);
    let variables = json!({"input":{
        "from":sender_key().public_key().to_string(), "to":receiver_key().public_key().to_string(),
        "fee":FEE.to_string()
    },"signature":{"field":field,"scalar":scalar}});
    let query = r#"mutation ($input:SendDelegationInput!, $signature:SignatureInput!) {
      sendDelegation(input:$input, signature:$signature) {
        __typename
        delegation {
          __typename id hash kind nonce amount fee feeToken token memo validUntil failureReason isDelegation
          from to
          source { publicKey nonce balance { total } }
          feePayer { publicKey }
          fromAccount { publicKey }
          receiver { publicKey nonce inferredNonce tokenSymbol receiptChainHash votingFor permissions { send }
            balance { total unknown liquid locked blockHeight stateHash } }
          toAccount { publicKey }
          ... on UserCommandDelegation { delegator { publicKey } delegatee { publicKey } }
          ... on UserCommandPayment { paymentOnly: id }
        }
      }
    }"#;
    let response = execute(&node, query, variables).await;
    assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
    assert_eq!(
        response["data"]["sendDelegation"]["__typename"],
        "SendDelegationPayload"
    );
    let result = &response["data"]["sendDelegation"]["delegation"];
    assert_eq!(result["__typename"], "UserCommandDelegation");
    assert!(result.get("paymentOnly").is_none());
    assert_eq!(result["kind"], "STAKE_DELEGATION");
    assert_eq!(result["nonce"], 0);
    assert_eq!(result["amount"], "0");
    assert_eq!(result["fee"], "10000000");
    assert_eq!(result["validUntil"], "4294967295");
    assert_eq!(result["isDelegation"], true);
    assert_eq!(result["failureReason"], Value::Null);
    assert_eq!(result["source"]["nonce"], "0");
    assert_eq!(result["source"]["balance"]["total"], "10000000000");
    for name in ["source", "feePayer", "fromAccount", "delegator"] {
        assert_eq!(
            result[name]["publicKey"],
            sender_key().public_key().to_string()
        );
    }
    for name in ["receiver", "toAccount", "delegatee"] {
        assert_eq!(
            result[name]["publicKey"],
            receiver_key().public_key().to_string()
        );
    }
    assert_eq!(result["from"], sender_key().public_key().to_string());
    assert_eq!(result["to"], receiver_key().public_key().to_string());
    for name in [
        "nonce",
        "inferredNonce",
        "tokenSymbol",
        "receiptChainHash",
        "votingFor",
        "permissions",
    ] {
        assert_eq!(result["receiver"][name], Value::Null, "receiver.{name}");
    }
    assert_eq!(
        result["receiver"]["balance"],
        json!({"total":"0","unknown":"0","liquid":null,"locked":null,"blockHeight":"0","stateHash":null})
    );
    // The returned transaction id must decode back to the command submitted.
    let returned = mina_p2p_messages::v2::MinaBaseSignedCommandStableV2::from_base64(
        result["id"].as_str().unwrap(),
    )
    .unwrap();
    let submitted = node.injected.lock().unwrap();
    assert_eq!(
        MinaBaseUserCommandStableV2::SignedCommand(returned),
        submitted[0]
    );
}

/// OCaml Account.Nonce.to_int retains all 32 unsigned bits; it does not cast
/// the high bit into a sign bit. JSON numbers above i32 use Juniper's Float
/// representation, which is exact for every UInt32.
#[tokio::test]
async fn command_nonce_above_i32_does_not_wrap_negative() {
    for nonce in [i32::MAX as u32, 2_147_483_648, u32::MAX - 1] {
        let node = FakeNode {
            ledger_nonce: Some(nonce),
            ..Default::default()
        };
        let response = execute(
            &node,
            SEND_PAYMENT,
            payment_variables(Some(nonce), nonce, NetworkId::MAINNET),
        )
        .await;
        assert_eq!(errors(&response), Vec::<String>::new(), "{response}");
        assert_eq!(
            response["data"]["sendPayment"]["payment"]["nonce"].as_f64(),
            Some(f64::from(nonce))
        );
        assert_eq!(injected_nonces(&node), vec![nonce]);
    }
}
