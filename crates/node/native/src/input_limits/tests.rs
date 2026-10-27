use super::*;
use mina_p2p_messages::v2;
use std::time::{Duration, Instant};

// OCaml fixtures already checked in under p2p-messages/src/v2/manual.rs.
pub(crate) const STATE: &str = "3NL7AkynW6hbDrhHTAht1GLG563Fo9fdcEQk1zEyy5XedC6aZTeB";
pub(crate) const LEDGER: &str = "jwrPvAMUNo3EKT2puUk5Fxz6B7apRAoKNTGpAA49t3TRSfzvdrL";
pub(crate) const SEED: &str = "2vajKi2Cxx58mByzxbJA3G6gYh1j2BoizW4zzoLcZa3kYECjhaXV";
pub(crate) const TOKEN: &str = "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf";
pub(crate) const KEY: &str = "B62qkUHaJUHERZuCHQhXCQ8xsGBqyYSgjQsKnKN5HhSJecakuJ4pYyk";
pub(crate) const SIG: &str = "7mXS9Y91bWtTYNKuDbxTuG18wUiZLHUySy9Ms8bPyAT9KNnME1q2nctwnvowJi2Y79dnsL18iVSCuaQF1ufUKwUZZKAXHqnF";
pub(crate) const MEMO_VALUE: &str = "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH";
// OCaml field value from graphql/zkapp.rs's authorization-kind fixture.
pub(crate) const FIELD: &str =
    "19951435866906059835892103359374709356309230417850637795098911039647240505427";

#[test]
fn format_maxima_accept_ocaml_encodings() {
    macro_rules! accepted {
        ($s:expr, $limit:expr, $ty:ty) => {{
            assert_eq!($s.len(), $limit);
            bounded($s, $limit).unwrap().parse::<$ty>().unwrap();
        }};
    }
    accepted!(STATE, STATE_HASH, v2::StateHash);
    accepted!(LEDGER, LEDGER_HASH, v2::LedgerHash);
    accepted!(SEED, EPOCH_SEED, v2::EpochSeed);
    accepted!(TOKEN, TOKEN_ID, v2::TokenIdKeyHash);
    accepted!(KEY, PUBLIC_KEY, mina_node::account::AccountPublicKey);
    accepted!(SIG, SIGNATURE, v2::Signature);
    // OCaml BestChain.json transaction hash.
    accepted!(
        "5JvEZkArzLHEtQS4aWsELuCHSNrUjb9CSwU8HthPjMsrsi2o2327",
        TRANSACTION_HASH,
        v2::TransactionHash
    );
    assert_eq!(MEMO_VALUE.len(), MEMO);
    assert_eq!(
        mina_p2p_messages::b58::decode(bounded(MEMO_VALUE, MEMO).unwrap(), 0x14)
            .unwrap()
            .len(),
        35
    );
    assert_eq!(FIELD.len(), FIELD_DECIMAL);
    mina_p2p_messages::bigint::BigInt::from_decimal(bounded(FIELD, FIELD_DECIMAL).unwrap())
        .unwrap();
    accepted!("4294967295", U32_DECIMAL, u32);
    accepted!("18446744073709551615", U64_DECIMAL, u64);
    accepted!(
        "Impossible",
        PERMISSION,
        v2::MinaBasePermissionsAuthRequiredStableV2
    );
    accepted!("Negative", SIGN, v2::SgnStableV1);
    let job = format!("{LEDGER}_{LEDGER}-{LEDGER}_{LEDGER}");
    assert_eq!(job.len(), JOB_ID);
    job_id(&job).unwrap();
}

#[test]
fn all_limits_reject_before_decoder_and_in_constant_time() {
    // Test the actual shared gate with a decoder sentinel. Never include input
    // construction in the time measurement. 1 ms matches the task's criterion.
    let long = "z".repeat(256 * 1024 + 1);
    for max in [
        STATE_HASH,
        LEDGER_HASH,
        EPOCH_SEED,
        TOKEN_ID,
        PUBLIC_KEY,
        TRANSACTION_HASH,
        SIGNATURE,
        MEMO,
        MEMO_TEXT,
        FIELD_DECIMAL,
        FIELD_HEX,
        SIGNATURE_HEX,
        U32_DECIMAL,
        U64_DECIMAL,
        PERMISSION,
        SIGN,
        JOB_ID,
        IDENTITY_KEY,
        PEER_ID,
        CHAIN_ID,
        HOST,
        VERIFICATION_KEY,
        SIGNED_COMMAND,
        OPAQUE_BASE64,
        SDP,
        OFFER_BASE58,
        255,
        6,
    ] {
        let at = "z".repeat(max);
        let over = "z".repeat(max + 1);
        assert!(bounded(&at, max).is_ok());
        for input in [&over, &long] {
            let start = Instant::now();
            let result = bounded(input, max).map(|_| panic!("decoder reached for limit {max}"));
            let elapsed = start.elapsed();
            assert_eq!(result.unwrap_err().max, max);
            assert!(elapsed < Duration::from_millis(1), "{max}: {elapsed:?}");
        }
    }
}

#[test]
fn payment_json_is_checked_before_typed_deserialization() {
    let valid = serde_json::json!({"from": KEY, "to": KEY, "memo": "x".repeat(MEMO_TEXT),
        "signature_field": FIELD, "signature_scalar": format!("0x{}", "00".repeat(32)),
        "fee": 1, "amount": 1, "nonce": 0, "valid_until": 0});
    assert!(serde_json::from_value::<Payment>(valid.clone()).is_ok());
    // Text memos already have a 32-byte O(1) check in Memo::create_from_str_checked.
    // Keep their existing conversion error (HTTP 500), rather than moving them
    // into serde's 422 envelope.
    let mut long_memo = valid.clone();
    long_memo["memo"] = "x".repeat(65_000).into();
    let payment: Payment = serde_json::from_value(long_memo).unwrap();
    assert!(v2::MinaBaseUserCommandStableV2::try_from(payment.0).is_err());
    for (field, max) in [
        ("from", PUBLIC_KEY),
        ("to", PUBLIC_KEY),
        ("signature_field", FIELD_DECIMAL),
        ("signature_scalar", FIELD_HEX),
    ] {
        for len in [max + 1, 65_000] {
            let mut value = valid.clone();
            value[field] = if field == "signature_scalar" {
                format!("0x{}", "f".repeat(len - 2))
            } else {
                "z".repeat(len)
            }
            .into();
            let err = serde_json::from_value::<Payment>(value).err().unwrap();
            assert!(err.to_string().contains("maximum length"), "{field}: {err}");
        }
    }
}

#[test]
fn job_components_cannot_borrow_other_components_budget() {
    let id = format!("{}_a-a_a", "z".repeat(LEDGER_HASH + 1));
    assert!(id.len() < JOB_ID);
    assert!(job_id(&id).is_err());
    assert!(serde_json::from_value::<JobId>(id.into()).is_err());
}

pub(crate) fn offer_fixture() -> serde_json::Value {
    // Native live public identity recorded in private implementation notes:
    // 12D3KooWRi8fzKr884rvBafaeNJR8YHqx9seXSyfe4vy8vTrPnKq.
    // Its raw Ed25519 bytes encoded independently with Python integer base58
    // and hashlib's double SHA-256 checksum, versions 0x16 and 0x2f.
    serde_json::json!({
        "identity_pub_key": "mZNbt6wKxbXiMU8NFX7K6ikXwbwfX2AGsn25NC6wrC9kQzgJ1k",
        "target_peer_id": "2c9z1gKjdbWC5rzm2k36A7i7Rnztv863wrYiB7PWoEMr5swaHBp",
        "chain_id": mina_core::MAINNET_CHAIN_ID.to_string(),
        "host": {"Domain": format!("{}.{}.{}.{}.", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61))},
        "sdp": "x".repeat(SDP), "listen_port": null
    })
}

#[test]
fn signaling_strings_accept_the_boundary_before_typed_deserialization() {
    let valid = offer_fixture();
    let offer: Offer = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(offer.0.sdp.len(), SDP);
    for (pointer, max) in [
        ("/identity_pub_key", IDENTITY_KEY),
        ("/target_peer_id", PEER_ID),
        ("/chain_id", CHAIN_ID),
        ("/host/Domain", HOST),
        ("/sdp", SDP),
    ] {
        assert_eq!(valid.pointer(pointer).unwrap().as_str().unwrap().len(), max);
        for len in [max + 1, 256 * 1024] {
            let mut value = valid.clone();
            *value.pointer_mut(pointer).unwrap() = "z".repeat(len).into();
            let err = serde_json::from_value::<Offer>(value).err().unwrap();
            assert!(
                err.to_string().contains("maximum length"),
                "{pointer}: {err}"
            );
        }
    }
}

#[test]
fn query_integer_strings_have_a_width_limit() {
    #[derive(serde::Deserialize)]
    struct Query {
        #[serde(deserialize_with = "optional_usize")]
        limit: Option<usize>,
        #[serde(deserialize_with = "query_usize")]
        offset: usize,
    }
    let max = usize::MAX.to_string();
    let q: Query = serde_json::from_value(serde_json::json!({"limit":max,"offset":max})).unwrap();
    assert_eq!(q.limit, Some(usize::MAX));
    assert_eq!(q.offset, usize::MAX);
    for field in ["limit", "offset"] {
        for len in [max.len() + 1, 65_000] {
            let mut value = serde_json::json!({"limit":"0","offset":"0"});
            value[field] = "0".repeat(len).into();
            assert!(serde_json::from_value::<Query>(value)
                .err()
                .unwrap()
                .to_string()
                .contains("maximum length"));
        }
    }
}

#[test]
fn signed_command_base64_maximum_follows_ocaml_binprot_widths() {
    // Derived independently from send_tests.rs OCAML_ID: retain its live keys,
    // memo and signature bytes; replace fee/amount with binprot int64(2^32)
    // and nonce/slot with int32(65536), using Python struct.pack. This is a
    // parsing fixture; changing the payload does not preserve its signature.
    const ID: &str = "AvwAAAAAAQAAAHP10FMEjBj0pNCcxyF+SVCqBGRsn920djOWlOHOFOo8AP0AAAEAAP0AAAEAIgEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWBnuHohCvlilq9xSmsYOiGw9Yx/TZqgW3lvtrUh48TcB/AAAAAABAAAAc/XQUwSMGPSk0JzHIX5JUKoEZGyf3bR2M5aU4c4U6jwAq4MQ8MzRo0tXRLOVUMjH0MIcR6JlAiHtz/NDSbM0nRGu75ZN2s+u/9vXAmD9LqjDL+k+X1XHyS7dKrUaBdeDNg==";
    assert_eq!(ID.len(), SIGNED_COMMAND);
    let command =
        v2::MinaBaseSignedCommandStableV2::from_base64(bounded(ID, SIGNED_COMMAND).unwrap())
            .unwrap();
    assert_eq!(command.payload.common.fee.as_u64(), 1 << 32);
    assert_eq!(command.payload.common.nonce.as_u32(), 65536);
}
