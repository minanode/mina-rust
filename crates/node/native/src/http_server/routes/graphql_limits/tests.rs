use super::*;
use crate::graphql::{Mutation, Query};
use juniper::EmptySubscription;
use serde_json::{json, Value};

fn schema() -> Schema {
    Schema::new(Query, Mutation, EmptySubscription::new())
}
fn request(query: &str) -> GraphQLBatchRequest {
    GraphQLBatchRequest::Single(GraphQLRequest::new(query.to_owned(), None, None))
}
fn rejected(query: &str, error: &str) {
    let result = check(&request(query), &schema(), PRIVATE).unwrap_err();
    assert!(result.contains(error), "{result}");
}

#[test]
fn alias_flood_and_nested_pages_are_rejected() {
    let flood = format!(
        "{{ {} }}",
        (0..1000)
            .map(|i| format!("v{i}: verification {{ verifiedSinceStart }}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert!(flood.len() < PUBLIC.query_bytes);
    rejected(&flood, "root field count");
    let pages = format!(
        "{{ verification {{ {} }} }}",
        (0..9)
            .map(|i| format!("p{i}: receipts {{ height }}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    rejected(&pages, "receipt page count");
    let fields = format!(
        "{{ verification {{ {} }} }}",
        (0..1024)
            .map(|i| format!("v{i}: verifiedSinceStart"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    rejected(&fields, "selection count");
}

#[test]
fn depth_fragments_and_parser_inputs_are_bounded() {
    rejected(
        &format!(
            "{} version {}",
            "{ f ".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        ),
        "depth",
    );
    rejected(
        &format!("{} version {}", "{ f ".repeat(1000), "}".repeat(1000)),
        "syntax nesting",
    );
    rejected(
        "{ ...A } fragment A on Query { ...B } fragment B on Query { ...A }",
        "Cyclic",
    );
    let mut query = "{ ...F0 }".to_owned();
    for i in 0..12 {
        query.push_str(&format!(
            " fragment F{i} on Query {{ ...F{} ...F{} }}",
            i + 1,
            i + 1
        ));
    }
    query.push_str(" fragment F12 on Query { verification { verifiedSinceStart } }");
    assert!(check(&request(&query), &schema(), PRIVATE).is_err());
    // A compact fragment chain must not bypass the expanded field depth limit.
    let mut query = "{ ...F0 }".to_owned();
    for i in 0..MAX_DEPTH + 1 {
        query.push_str(&format!(
            " fragment F{i} on Query {{ f {{ ...F{} }} }}",
            i + 1
        ));
    }
    query.push_str(&format!(
        " fragment F{} on Query {{ version }}",
        MAX_DEPTH + 1
    ));
    rejected(&query, "depth");
    rejected(
        &format!("{{version}} #{}", "x".repeat(PRIVATE.query_bytes)),
        "bytes",
    );
    rejected(
        &format!("{{ {} }}", "a ".repeat(PRIVATE.structure_tokens)),
        "tokens outside arguments",
    );
    // Argument values have the larger allowance, but not an unbounded one.
    rejected(
        &format!("{{ f(a: [{}]) }}", "0 ".repeat(PRIVATE.tokens)),
        &format!("exceeds {} tokens", PRIVATE.tokens),
    );
    assert!(check(&request("{ version # {{{[[[\n }"), &schema(), PRIVATE).is_ok());
}

#[test]
fn batch_allowances_are_aggregate() {
    let small = GraphQLRequest::new("{ version }".into(), None, None);
    assert!(check(
        &GraphQLBatchRequest::Batch(vec![small; MAX_BATCH + 1]),
        &schema(),
        PRIVATE
    )
    .is_err());
    let query = format!(
        "{{ {} }}",
        (0..4)
            .map(|i| format!("v{i}: version"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let batch = GraphQLBatchRequest::Batch(vec![GraphQLRequest::new(query, None, None); 9]);
    assert!(check(&batch, &schema(), PRIVATE)
        .unwrap_err()
        .contains("root field count"));
    // Juniper's parse time is quadratic in a document's tokens, so a batch
    // shares one token allowance: two queries of 60% each are refused.
    let values = format!("{{ f(a: [{}]) }}", "0 ".repeat(PRIVATE.tokens * 6 / 10));
    let one = GraphQLRequest::new(values, None, None);
    let result = check(
        &GraphQLBatchRequest::Batch(vec![one.clone()]),
        &schema(),
        PRIVATE,
    );
    assert!(!result.is_err_and(|e| e.contains("tokens")));
    let result = check(
        &GraphQLBatchRequest::Batch(vec![one; 2]),
        &schema(),
        PRIVATE,
    )
    .unwrap_err();
    assert!(
        result.contains(&format!("exceeds {} tokens", PRIVATE.tokens)),
        "{result}"
    );
}

#[test]
fn dashboard_and_p7_operations_fit_with_headroom() {
    let schema = schema();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let chain =
        std::fs::read_to_string(root.join("frontend/src/app/features/chain/chain.service.ts"))
            .unwrap();
    let block_fields = chain
        .split("const BLOCK_FIELDS = `")
        .nth(1)
        .unwrap()
        .split('`')
        .next()
        .unwrap();
    let mut queries = vec![];
    for template in chain.split('`').skip(1).step_by(2) {
        if template.trim_start().starts_with("query ") {
            queries.push(template.replace("${BLOCK_FIELDS}", block_fields));
        }
    }
    // Topology now uses REST /state/peers (R28), so only the three chain
    // operations above are frontend GraphQL queries.
    assert_eq!(queries.len(), 3);
    queries.push("{ verification { capacity verifiedSinceStart failuresSinceStart recentFailureCount medianProofTimeMs lastVerified { stateHash applied } receipts(limit: 64) { stateHash applied blockProof { status } } } }".into());
    let dashboard_count = queries.len();
    for file in ["auro", "o1js", "rosetta", "sdk"] {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/files/graphql/operations/{file}.graphql")),
        )
        .unwrap();
        let mut current = String::new();
        for line in source.lines() {
            if line.starts_with("query ") || line.starts_with("mutation ") {
                if !current.is_empty() {
                    queries.push(std::mem::take(&mut current));
                }
            }
            if !current.is_empty() || line.starts_with("query ") || line.starts_with("mutation ") {
                current.push_str(line);
                current.push('\n');
            }
        }
        if !current.is_empty() {
            queries.push(current);
        }
    }
    let mut max_bytes = 0;
    let mut max_fields = 0;
    let mut max_depth = 0;
    let mut max_roots = 0;
    for query in &queries {
        let mut budget = Budget::default();
        check_query(
            &GraphQLRequest::new(query.clone(), None, None),
            &schema,
            PUBLIC,
            &mut budget,
        )
        .unwrap_or_else(|e| panic!("{e}: {query}"));
        assert!(query.len() < PUBLIC.query_bytes / 2);
        assert!(budget.selections < MAX_SELECTIONS / 2);
        assert!(budget.root_fields < MAX_ROOT_FIELDS / 2);
        assert!(budget.depth <= MAX_DEPTH / 2);
        max_bytes = max_bytes.max(query.len());
        max_fields = max_fields.max(budget.selections);
        max_depth = max_depth.max(budget.depth);
        max_roots = max_roots.max(budget.root_fields);
    }
    println!("{} dashboard/telemetry + {} P7 operations: max query bytes={max_bytes}, selections={max_fields}, depth={max_depth}, roots={max_roots}", dashboard_count, queries.len()-dashboard_count);
}

/// The selection o1js's `sendZkappQuery` asks for after the command.
const O1JS_SELECTION: &str = "  }) {
    zkapp {
      hash
      id
      failureReason {
        failures
        index
      }
      zkappCommand {
        memo
        feePayer {
          body {
            publicKey
          }
        }
        accountUpdates {
          body {
            publicKey
            useFullCommitment
            incrementNonce
          }
        }
      }
    }
  }
}
";

/// The mutation o1js 3.1.0 sends for a command (`sendZkappQuery` in
/// `lib/mina/v1/graphql.js`): the command's JSON inline, indented by two
/// spaces, with `removeJsonQuotes` taking the quotes off its keys.
pub(in crate::http_server::routes) fn o1js_send_zkapp(command: &Value) -> String {
    let mut query = String::from("mutation {\n  sendZkapp(input: {\n    zkappCommand: ");
    for (i, line) in serde_json::to_string_pretty(command)
        .unwrap()
        .lines()
        .enumerate()
    {
        if i > 0 {
            query.push('\n');
        }
        let text = line.trim_start();
        match text.strip_prefix('"').and_then(|t| t.split_once("\":")) {
            Some((key, rest)) if !key.contains(|c: char| c.is_whitespace() || c == '"') => {
                query.push_str(&line[..line.len() - text.len()]);
                query.push_str(key);
                query.push(':');
                query.push_str(rest);
            }
            _ => query.push_str(line),
        }
    }
    query.push('\n');
    query.push_str(O1JS_SELECTION);
    query
}

/// The HTTP body o1js sends (`makeGraphqlRequest` in `lib/mina/v1/fetch.js`).
pub(in crate::http_server::routes) fn o1js_body(query: &str) -> String {
    json!({ "operationName": null, "query": query, "variables": {} }).to_string()
}

/// The largest base field element, p - 1: 77 decimal digits.
const FIELD: &str = "28948022309329048855892746252171976963363056481941560715954676764349967630336";
/// The longest valid zkApp proof, base64 of its S-expression: one chunk
/// (OCaml's side-loaded verifier allows no more), every optional lookup and
/// custom-gate evaluation, a joint combiner, two previous proofs.
const MAX_PROOF_BASE64: usize = 38_728;
const PUBLIC_KEY: usize = 55;
const SIGNATURE: usize = 96;
const U64_MAX: &str = "18446744073709551615";
const U32_MAX: &str = "4294967295";

fn range(value: &str) -> Value {
    json!({ "lower": value, "upper": value })
}

fn epoch_data(ledger: Value, seed: Value, length: Value) -> Value {
    json!({
        "ledger": { "hash": ledger, "totalCurrency": length.clone() },
        "seed": seed.clone(),
        "startCheckpoint": seed.clone(),
        "lockCheckpoint": seed,
        "epochLength": length,
    })
}

/// An account update as o1js writes it, with every optional field null,
/// as most of the PK1 bridge's are.
fn sparse_update(authorization: Value) -> Value {
    let network = json!({
        "snarkedLedgerHash": null,
        "blockchainLength": null,
        "minWindowDensity": null,
        "totalCurrency": null,
        "globalSlotSinceGenesis": null,
        "stakingEpochData": epoch_data(Value::Null, Value::Null, Value::Null),
        "nextEpochData": epoch_data(Value::Null, Value::Null, Value::Null),
    });
    let account = json!({
        "balance": null, "nonce": null, "receiptChainHash": null, "delegate": null,
        "state": vec![Value::Null; 32], "actionState": null, "provedState": null, "isNew": null,
    });
    let update = json!({
        "appState": vec![Value::Null; 32], "delegate": null, "verificationKey": null,
        "permissions": null, "zkappUri": null, "tokenSymbol": null, "timing": null,
        "votingFor": null,
    });
    json!({
        "body": {
            "publicKey": format!("B62q{}", "x".repeat(PUBLIC_KEY - 4)),
            "tokenId": "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf",
            "update": update,
            "balanceChange": { "magnitude": "0", "sgn": "Positive" },
            "incrementNonce": false,
            "events": [],
            "actions": [],
            "callData": "0",
            "callDepth": 0,
            "preconditions": { "network": network, "account": account, "validWhile": null },
            "useFullCommitment": false,
            "implicitAccountCreationFee": false,
            "mayUseToken": { "parentsOwnToken": false, "inheritFromParent": false },
            "authorizationKind": { "isSigned": false, "isProved": false, "verificationKeyHash": FIELD },
        },
        "authorization": authorization,
    })
}

fn proof(len: usize) -> Value {
    json!({ "proof": "K".repeat(len), "signature": null })
}

fn signature() -> Value {
    json!({ "proof": null, "signature": format!("7m{}", "x".repeat(SIGNATURE - 2)) })
}

/// An account update with every field at its longest encoding (the input
/// length limits of `crate::input_limits`): 32 app-state fields and 32
/// precondition fields, a verification key, a 255-byte zkApp URI and a
/// 6-byte token symbol of control bytes (each escaped as six characters),
/// every network precondition.
fn full_update(authorization: Value) -> Value {
    let mut update = sparse_update(authorization);
    let body = &mut update["body"];
    let (key, ledger, hash) = (body["publicKey"].clone(), "j".repeat(51), "3".repeat(52));
    let impossible = "Impossible";
    body["update"] = json!({
        "appState": vec![FIELD; 32],
        "delegate": key.clone(),
        "verificationKey": { "data": "A".repeat(2396), "hash": FIELD },
        "permissions": {
            "editState": impossible, "access": impossible, "send": impossible,
            "receive": impossible, "setDelegate": impossible, "setPermissions": impossible,
            "setVerificationKey": { "auth": impossible, "txnVersion": "4" },
            "setZkappUri": impossible, "editActionState": impossible,
            "setTokenSymbol": impossible, "incrementNonce": impossible,
            "setVotingFor": impossible, "setTiming": impossible,
        },
        "zkappUri": "\u{1}".repeat(255),
        "tokenSymbol": "\u{1}".repeat(6),
        "timing": {
            "initialMinimumBalance": U64_MAX, "cliffTime": U32_MAX, "cliffAmount": U64_MAX,
            "vestingPeriod": U32_MAX, "vestingIncrement": U64_MAX,
        },
        "votingFor": hash.clone(),
    });
    body["tokenId"] = json!("w".repeat(50));
    body["balanceChange"] = json!({ "magnitude": U64_MAX, "sgn": "Negative" });
    body["callData"] = json!(FIELD);
    body["callDepth"] = json!(15);
    let mut epoch = epoch_data(json!(ledger), json!(hash), range(U32_MAX));
    epoch["ledger"]["totalCurrency"] = range(U64_MAX);
    body["preconditions"] = json!({
        "network": {
            "snarkedLedgerHash": ledger,
            "blockchainLength": range(U32_MAX),
            "minWindowDensity": range(U32_MAX),
            "totalCurrency": range(U64_MAX),
            "globalSlotSinceGenesis": range(U32_MAX),
            "stakingEpochData": epoch.clone(),
            "nextEpochData": epoch,
        },
        "account": {
            "balance": range(U64_MAX), "nonce": range(U32_MAX), "receiptChainHash": hash,
            "delegate": key, "state": vec![FIELD; 32], "actionState": FIELD,
            "provedState": false, "isNew": false,
        },
        "validWhile": range(U32_MAX),
    });
    update
}

fn command(fee: &str, valid_until: Value, nonce: &str, updates: Vec<Value>) -> Value {
    json!({
        "feePayer": {
            "body": {
                "publicKey": format!("B62q{}", "x".repeat(PUBLIC_KEY - 4)),
                "fee": fee, "validUntil": valid_until, "nonce": nonce,
            },
            "authorization": format!("7m{}", "x".repeat(SIGNATURE - 2)),
        },
        "accountUpdates": updates,
        "memo": format!("E4{}", "x".repeat(51)),
    })
}

/// The largest commands a Mesa node accepts: 16 segments
/// (`max_zkapp_segment_per_transaction`), where a proof takes a segment and
/// two signed or unauthorized updates (the fee payer included) share one;
/// 1024 event and 1024 action elements, one per event. Every field is at
/// its longest.
pub(in crate::http_server::routes) fn worst_case(proved: usize, signed: usize) -> Value {
    let mut updates: Vec<_> = (0..signed)
        .map(|_| full_update(signature()))
        .chain((0..proved).map(|_| {
            let mut update = full_update(proof(MAX_PROOF_BASE64));
            update["body"]["authorizationKind"]["isProved"] = json!(true);
            update
        }))
        .collect();
    updates[0]["body"]["events"] = json!(vec![[FIELD]; 1024]);
    updates[0]["body"]["actions"] = json!(vec![[FIELD]; 1024]);
    command(U64_MAX, json!(U32_MAX), U32_MAX, updates)
}

fn tokens(query: &str) -> usize {
    juniper::parser::Lexer::new(query).count()
}

/// PK1's bridge mutation, 86,814 bytes, was refused at the earlier 64 KiB
/// limit although a public OCaml node accepted it. This one has its shape:
/// five updates, two proved (32,248-byte proofs), one setting a
/// verification key and permissions, four setting all 32 app-state fields.
#[test]
fn a_pk1_bridge_shaped_zkapp_mutation_is_admitted() {
    let mut updates = vec![
        sparse_update(signature()),
        sparse_update(proof(32_248)),
        full_update(signature()),
        sparse_update(proof(32_248)),
        sparse_update(json!({ "proof": null, "signature": null })),
    ];
    for update in &mut updates[..4] {
        update["body"]["update"]["appState"] = json!(vec![FIELD; 32]);
    }
    let full = updates[2]["body"]["update"].clone();
    updates[2]["body"]["update"] = json!({
        "appState": full["appState"], "delegate": null,
        "verificationKey": full["verificationKey"], "permissions": full["permissions"],
        "zkappUri": null, "tokenSymbol": null, "timing": null, "votingFor": null,
    });
    updates[2]["body"]["preconditions"] = updates[0]["body"]["preconditions"].clone();
    let query = o1js_send_zkapp(&command("100000000", Value::Null, "2", updates));
    println!(
        "bridge-shaped: {} query bytes, {} tokens",
        query.len(),
        tokens(&query)
    );
    assert!(query.len() > PUBLIC.query_bytes);
    check(&request(&query), &schema(), PRIVATE).unwrap();
    assert!(check(&request(&query), &schema(), PUBLIC)
        .unwrap_err()
        .contains("bytes"));
}

/// The largest commands o1js builds pass with a margin of a fifth: the most
/// bytes (15 proofs and a signed update paired with the fee payer) and the
/// most tokens (31 signed updates, two to a segment). The sizes are the
/// arithmetic of private implementation notes.
#[test]
fn the_largest_zkapp_mutations_are_admitted_with_a_margin() {
    let schema = schema();
    for (name, command, bytes, body_bytes, token_count) in [
        (
            "15 proved + 1 signed",
            worst_case(15, 1),
            1_060_965,
            1_082_925,
            13_021,
        ),
        ("31 signed", worst_case(0, 31), 707_460, 740_310, 19_396),
    ] {
        let query = o1js_send_zkapp(&command);
        let body = o1js_body(&query);
        assert_eq!(
            (query.len(), body.len(), tokens(&query)),
            (bytes, body_bytes, token_count),
            "{name}"
        );
        assert!(query.len() * 6 / 5 < PRIVATE.query_bytes, "{name}");
        assert!(body.len() * 6 / 5 < PRIVATE.body_bytes, "{name}");
        assert!(token_count * 6 / 5 < PRIVATE.tokens, "{name}");
        // The variables form: o1js does not use it, other clients do.
        let variables = json!({
            "query": "mutation($input: SendZkappInput!) { sendZkapp(input: $input) { zkapp { hash } } }",
            "variables": { "input": { "zkappCommand": command } },
        })
        .to_string();
        assert!(variables.len() * 6 / 5 < PRIVATE.body_bytes, "{name}");

        let started = std::time::Instant::now();
        let parsed: GraphQLBatchRequest = serde_json::from_str(&body).unwrap();
        let decoded = started.elapsed();
        let started = std::time::Instant::now();
        check(&parsed, &schema, PRIVATE).unwrap_or_else(|e| panic!("{name}: {e}"));
        println!(
            "{name}: {} query bytes, {} body bytes, {token_count} tokens, \
             {} variables-form body bytes; JSON decode {decoded:?}, admission {:?}",
            query.len(),
            body.len(),
            variables.len(),
            started.elapsed()
        );
    }
}

/// A public node refuses mutations, so it keeps the earlier limits: 64 KiB
/// of query text and 8192 tokens, argument values included.
#[test]
fn a_public_node_keeps_the_earlier_limits() {
    let values = format!("{{ f(a: [{}]) }}", "0 ".repeat(PUBLIC.tokens));
    let result = check(&request(&values), &schema(), PUBLIC).unwrap_err();
    assert!(result.contains("8192 tokens"), "{result}");
    let result = check(&request(&values), &schema(), PRIVATE);
    assert!(!result.is_err_and(|e| e.contains("tokens")));
    let long = format!("{{version}} #{}", "x".repeat(PUBLIC.query_bytes));
    let result = check(&request(&long), &schema(), PUBLIC).unwrap_err();
    assert!(result.contains("65536 bytes"), "{result}");
    assert_eq!(
        (PUBLIC.body_bytes, PUBLIC.query_bytes, PUBLIC.tokens),
        (256 * 1024, 64 * 1024, 8192)
    );
}
