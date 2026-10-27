//! Input boundaries exercised through the actual GraphQL conversions.
use super::*;
use crate::input_limits::{self as caps, tests::*};
use juniper::FromInputValue;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn zkapp_fixture() -> Value {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../../../../tests/files/zkapps/with_sig_auth_graphql.json"
    ))
    .unwrap();
    // One independent update is sufficient to exercise every nested field.
    v["accountUpdates"].as_array_mut().unwrap().truncate(1);
    let body = &mut v["accountUpdates"][0]["body"];
    body["update"]["permissions"] = json!({
        "editState":"None", "access":"None", "send":"None", "receive":"None",
        "setDelegate":"None", "setPermissions":"None", "setVerificationKey":{"auth":"None","txnVersion":"4"},
        "setZkappUri":"None", "editActionState":"None", "setTokenSymbol":"None", "setTiming":"None",
        "setVotingFor":"None", "incrementNonce":"None"
    });
    body["update"]["timing"] = json!({"initialMinimumBalance":"0","cliffTime":0,"cliffAmount":"0","vestingPeriod":1,"vestingIncrement":"0"});
    body["events"] = json!([["0"]]);
    body["actions"] = json!([["0"]]);
    body["preconditions"]["account"]["balance"] = json!({"lower":"0","upper":"0"});
    body["preconditions"]["account"]["nonce"] = json!({"lower":"0","upper":"0"});
    body["preconditions"]["validWhile"] = json!({"lower":"0","upper":"0"});
    let network = &mut body["preconditions"]["network"];
    for field in [
        "blockchainLength",
        "minWindowDensity",
        "totalCurrency",
        "globalSlotSinceGenesis",
    ] {
        network[field] = json!({"lower":"0","upper":"0"});
    }
    for epoch in ["stakingEpochData", "nextEpochData"] {
        network[epoch]["epochLength"] = json!({"lower":"0","upper":"0"});
        network[epoch]["ledger"]["totalCurrency"] = json!({"lower":"0","upper":"0"});
    }
    v
}

fn convert(value: Value) -> Result<MinaBaseUserCommandStableV2, ConversionError> {
    let input: juniper::InputValue = serde_json::from_value(value).unwrap();
    zkapp::InputGraphQLZkappCommand::from_input_value(&input)
        .unwrap()
        .try_into()
}

#[test]
fn nested_zkapp_inputs_accept_maximum_and_reject_one_over() {
    let body = "/accountUpdates/0/body";
    let mut cases: Vec<(String, String, usize)> = vec![
        ("/memo".into(), MEMO_VALUE.into(), caps::MEMO),
        (
            "/feePayer/authorization".into(),
            SIG.into(),
            caps::SIGNATURE,
        ),
        (
            "/feePayer/body/publicKey".into(),
            KEY.into(),
            caps::PUBLIC_KEY,
        ),
        (
            "/feePayer/body/fee".into(),
            u64::MAX.to_string(),
            caps::U64_DECIMAL,
        ),
        (
            "/feePayer/body/nonce".into(),
            u32::MAX.to_string(),
            caps::U32_DECIMAL,
        ),
        (
            "/feePayer/body/validUntil".into(),
            u32::MAX.to_string(),
            caps::U32_DECIMAL,
        ),
        (
            "/accountUpdates/0/authorization/signature".into(),
            SIG.into(),
            caps::SIGNATURE,
        ),
    ];
    for (path, valid, cap) in [
        ("publicKey", KEY, caps::PUBLIC_KEY),
        ("tokenId", TOKEN, caps::TOKEN_ID),
        ("update/delegate", KEY, caps::PUBLIC_KEY),
        ("update/votingFor", STATE, caps::STATE_HASH),
        ("callData", FIELD, caps::FIELD_DECIMAL),
        ("update/appState/0", FIELD, caps::FIELD_DECIMAL),
        ("events/0/0", FIELD, caps::FIELD_DECIMAL),
        ("actions/0/0", FIELD, caps::FIELD_DECIMAL),
        ("preconditions/account/delegate", KEY, caps::PUBLIC_KEY),
        ("preconditions/account/state/0", FIELD, caps::FIELD_DECIMAL),
        (
            "preconditions/account/actionState",
            FIELD,
            caps::FIELD_DECIMAL,
        ),
        (
            "preconditions/account/receiptChainHash",
            FIELD,
            caps::FIELD_DECIMAL,
        ),
        (
            "preconditions/network/snarkedLedgerHash",
            LEDGER,
            caps::LEDGER_HASH,
        ),
        ("balanceChange/sgn", "Negative", caps::SIGN),
        (
            "balanceChange/magnitude",
            "18446744073709551615",
            caps::U64_DECIMAL,
        ),
    ] {
        cases.push((format!("{body}/{path}"), valid.into(), cap));
    }
    for (path, cap) in [("update/zkappUri", 255), ("update/tokenSymbol", 6)] {
        cases.push((format!("{body}/{path}"), "x".repeat(cap), cap));
    }
    for epoch in ["stakingEpochData", "nextEpochData"] {
        for (field, valid, cap) in [
            ("seed", SEED, caps::EPOCH_SEED),
            ("startCheckpoint", STATE, caps::STATE_HASH),
            ("lockCheckpoint", STATE, caps::STATE_HASH),
            ("ledger/hash", LEDGER, caps::LEDGER_HASH),
        ] {
            cases.push((
                format!("{body}/preconditions/network/{epoch}/{field}"),
                valid.into(),
                cap,
            ));
        }
        for end in ["lower", "upper"] {
            cases.push((
                format!("{body}/preconditions/network/{epoch}/epochLength/{end}"),
                u32::MAX.to_string(),
                caps::U32_DECIMAL,
            ));
            cases.push((
                format!("{body}/preconditions/network/{epoch}/ledger/totalCurrency/{end}"),
                u64::MAX.to_string(),
                caps::U64_DECIMAL,
            ));
        }
    }
    for (field, cap, valid) in [
        ("account/balance", caps::U64_DECIMAL, u64::MAX.to_string()),
        ("account/nonce", caps::U32_DECIMAL, u32::MAX.to_string()),
        ("validWhile", caps::U32_DECIMAL, u32::MAX.to_string()),
        (
            "network/blockchainLength",
            caps::U32_DECIMAL,
            u32::MAX.to_string(),
        ),
        (
            "network/minWindowDensity",
            caps::U32_DECIMAL,
            u32::MAX.to_string(),
        ),
        (
            "network/globalSlotSinceGenesis",
            caps::U32_DECIMAL,
            u32::MAX.to_string(),
        ),
        (
            "network/totalCurrency",
            caps::U64_DECIMAL,
            u64::MAX.to_string(),
        ),
    ] {
        for end in ["lower", "upper"] {
            cases.push((
                format!("{body}/preconditions/{field}/{end}"),
                valid.clone(),
                cap,
            ));
        }
    }
    for field in [
        "editState",
        "access",
        "send",
        "receive",
        "setDelegate",
        "setPermissions",
        "setVerificationKey/auth",
        "setZkappUri",
        "editActionState",
        "setTokenSymbol",
        "setTiming",
        "setVotingFor",
        "incrementNonce",
    ] {
        cases.push((
            format!("{body}/update/permissions/{field}"),
            "Impossible".into(),
            caps::PERMISSION,
        ));
    }
    cases.push((
        format!("{body}/update/permissions/setVerificationKey/txnVersion"),
        u32::MAX.to_string(),
        caps::U32_DECIMAL,
    ));
    for field in ["initialMinimumBalance", "cliffAmount", "vestingIncrement"] {
        cases.push((
            format!("{body}/update/timing/{field}"),
            u64::MAX.to_string(),
            caps::U64_DECIMAL,
        ));
    }
    let mut worst = Duration::ZERO;
    for (path, valid, cap) in &cases {
        assert_eq!(valid.len(), *cap, "{path}");
        let mut value = zkapp_fixture();
        *value.pointer_mut(path).unwrap() = valid.clone().into();
        convert(value).unwrap_or_else(|err| panic!("valid {path}: {err}"));
        for len in [cap + 1, 65_000] {
            let mut value = zkapp_fixture();
            *value.pointer_mut(path).unwrap() = "z".repeat(len).into();
            // Coercion copies strings; measure just conversion/decoding.
            let input: juniper::InputValue = serde_json::from_value(value).unwrap();
            let input = zkapp::InputGraphQLZkappCommand::from_input_value(&input).unwrap();
            let start = Instant::now();
            let result: Result<MinaBaseUserCommandStableV2, _> = input.try_into();
            let elapsed = start.elapsed();
            worst = worst.max(elapsed);
            assert!(
                matches!(result, Err(ConversionError::InputTooLong(_))),
                "{path}: {result:?}"
            );
            assert!(elapsed < Duration::from_millis(100), "{path}: {elapsed:?}");
        }
    }
    println!(
        "{} nested input sites, max/one-over/65k; worst conversion {worst:?}",
        cases.len()
    );
    // Short malformed memos also return the existing conversion error envelope.
    let mut value = zkapp_fixture();
    value["memo"] = "not base58!".into();
    assert!(matches!(convert(value), Err(ConversionError::Base58(_))));
}

#[tokio::test]
async fn root_graphql_inputs_reject_before_rpc() {
    use mina_node::core::channels::mpsc;
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
    let context = Context::new(RpcSender::new(tx));
    let schema = juniper::RootNode::new(Query, Mutation, juniper::EmptySubscription::new());
    for (query, cap) in [
        ("query($v:ID!){transactionStatus(payment:$v)}", caps::SIGNED_COMMAND),
        ("query($v:ID!){transactionStatus(zkappTransaction:$v)}", caps::OPAQUE_BASE64),
        ("query($v:ID!){pooledUserCommands(ids:[$v]){hash}}", caps::SIGNED_COMMAND),
        ("query($v:ID!){pooledZkappCommands(ids:[$v]){hash}}", caps::OPAQUE_BASE64),
        ("query($v:String!){block(stateHash:$v){stateHash}}", caps::STATE_HASH),
        ("query($v:PublicKey!){account(publicKey:$v){publicKey}}", caps::PUBLIC_KEY),
        ("query($v:PublicKey!){accounts(publicKey:$v){publicKey}}", caps::PUBLIC_KEY),
        ("query($v:TokenId!){account(publicKey:\"B62qkUHaJUHERZuCHQhXCQ8xsGBqyYSgjQsKnKN5HhSJecakuJ4pYyk\",token:$v){publicKey}}", caps::TOKEN_ID),
        ("query($v:String!){pooledUserCommands(hashes:[$v]){hash}}", caps::TRANSACTION_HASH),
        ("query($v:String!){pooledZkappCommands(hashes:[$v]){hash}}", caps::TRANSACTION_HASH),
        ("query($v:PublicKey!){pooledUserCommands(publicKey:$v){hash}}", caps::PUBLIC_KEY),
        ("query($v:PublicKey!){pooledZkappCommands(publicKey:$v){hash}}", caps::PUBLIC_KEY),
    ] {
        for len in [cap + 1, 65_000.max(cap + 1)] {
            let variables = serde_json::from_value(json!({"v":"z".repeat(len)})).unwrap();
            let response = juniper::http::GraphQLRequest::new(query.into(), None, Some(variables)).execute(&schema, &context).await;
            let json = serde_json::to_value(response).unwrap();
            let errors = json["errors"].to_string();
            // Or OCaml's words for an argument that does not decode, which
            // come first for ids that are not Base64.
            assert!(
                ["maximum length", "Invalid transaction provided", "expected on field"]
                    .iter()
                    .any(|text| errors.contains(text)),
                "{query}: {errors}"
            );
            assert!(rx.try_recv().is_err(), "invalid input must not dispatch RPC");
        }
    }
}

#[test]
fn signature_fields_are_bounded_and_unicode_cannot_panic() {
    use user_command::UserCommandSignature;
    let signature = |raw, field, scalar| UserCommandSignature {
        raw_signature: raw,
        field,
        scalar,
    };
    // OCaml decimal fixture, valid at the maximum decimal length.
    mina_signer::Signature::try_from(signature(None, Some(FIELD.into()), Some(FIELD.into())))
        .unwrap();
    for field in [true, false] {
        for len in [caps::FIELD_DECIMAL + 1, 65_000] {
            let bad = "9".repeat(len);
            let input = if field {
                signature(None, Some(bad), Some(FIELD.into()))
            } else {
                signature(None, Some(FIELD.into()), Some(bad))
            };
            assert!(matches!(
                mina_signer::Signature::try_from(input),
                Err(ConversionError::InputTooLong(_))
            ));
        }
    }
    // Signature bytes from the same OCaml base58 fixture in manual.rs.
    mina_signer::Signature::try_from(signature(Some("d290f924705fb714e91fedb9bed77e85bce8f5d932c3f4d692b20e4c3e5f9a3343c2baffce9ab0c2391e2f3de8ac891633338d827e6fd4f269331c248029b106".into()), None, None)).unwrap();
    for raw in ["f".repeat(129), "f".repeat(65_000), "é".repeat(63)] {
        assert!(mina_signer::Signature::try_from(signature(Some(raw), None, None)).is_err());
    }
}
