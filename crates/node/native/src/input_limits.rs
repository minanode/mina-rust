//! HTTP input limits, checked before base conversion or string parsing.
//!
//! Fixed Base58Check limits count the version byte, binprot version tags,
//! payload and four checksum bytes.

#[derive(Debug, thiserror::Error)]
#[error("input exceeds maximum length of {max} bytes")]
pub struct InputTooLong {
    pub max: usize,
}

/// O(1), including for UTF-8. Call before decoding, scanning or copying input.
pub fn bounded(input: &str, max: usize) -> Result<&str, InputTooLong> {
    if input.len() > max {
        Err(InputTooLong { max })
    } else {
        Ok(input)
    }
}

// Base58Check digits of ((version + 1) * 256^(bytes - 1) - 1).
// Formats: p2p-messages/src/{lib.rs,b58.rs,v2/manual.rs}; the version
// byte makes these tighter than ceil(bytes * log(256) / log(58)).
pub const STATE_HASH: usize = 52; // 0x10 + (1 + 32) + 4
pub const LEDGER_HASH: usize = 51; // 0x05 + (1 + 32) + 4
pub const EPOCH_SEED: usize = 52; // 0x0d + (1 + 32) + 4
pub const TOKEN_ID: usize = 50; // 0x1c + 32 + 4 (no binprot version)
pub const PUBLIC_KEY: usize = 55; // 0xcb + (1 + 1 + 32 + 1) + 4
pub const TRANSACTION_HASH: usize = 52; // 0x1d + (length byte + 32) + 4
pub const SIGNATURE: usize = 96; // 0x9a + (1 + 32 + 32) + 4
pub const MEMO: usize = 53; // 0x14 + 34 + 4
pub const MEMO_TEXT: usize = 32; // tag + length + 32 bytes = 34
pub const FIELD_DECIMAL: usize = 77; // Pasta fields are 255 bits
pub const FIELD_HEX: usize = 66; // 0x and 32 bytes
pub const SIGNATURE_HEX: usize = 128; // two 32-byte fields
pub const U32_DECIMAL: usize = 10;
pub const U64_DECIMAL: usize = 20;
pub const PERMISSION: usize = 10; // "Impossible"
pub const SIGN: usize = 8; // "Positive" / "Negative"
pub const JOB_ID: usize = 4 * LEDGER_HASH + 3; // a_b-c_d
pub const IDENTITY_KEY: usize = 50; // 0x16 + 32 + 4
pub const PEER_ID: usize = 51; // 0x2f + 32 + 4
pub const CHAIN_ID: usize = 64; // 32-byte hex
pub const HOST: usize = 254; // DNS presentation name, including optional final dot
pub const SIGNED_COMMAND: usize = 308; // base64 of at most 229 binprot bytes
pub const VERIFICATION_KEY: usize = 2396; // base64(2 tags + 28*64 + 2 units)

// Variable-size formats have no finite encoding maximum. These are HTTP
// resource policies, not consensus rules. Opaque commands/proofs retain the
// existing GraphQL body budget; SDP uses a separate generous budget.
pub const OPAQUE_BASE64: usize = 256 * 1024;
pub const SDP: usize = 64 * 1024;
pub const OFFER_BASE58: usize = 8192; // larger offers must use JSON POST

/// Bound each ledger hash as well as the compound job ID, before FromStr.
pub fn job_id(input: &str) -> Result<mina_core::snark::SnarkJobId, ()> {
    bounded(input, JOB_ID).map_err(|_| ())?;
    for hash in input.split(['_', '-']) {
        bounded(hash, LEDGER_HASH).map_err(|_| ())?;
    }
    input.parse()
}

/// HTTP-only wrapper: keep axum's existing path/query rejection behavior.
pub struct JobId(pub mina_core::snark::SnarkJobId);
impl<'de> serde::Deserialize<'de> for JobId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let input = <String as serde::Deserialize>::deserialize(d)?;
        job_id(&input)
            .map(Self)
            .map_err(|_| serde::de::Error::custom("decode from str failed"))
    }
}

/// Validate raw JSON fields before the typed serde decoder can reach base58.
/// Using a Deserialize wrapper preserves Json/AssumeJson's 422 errors.
pub struct Payment(pub mina_node::rpc::RpcInjectPayment);
impl<'de> serde::Deserialize<'de> for Payment {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        for (name, max) in [("from", PUBLIC_KEY), ("to", PUBLIC_KEY)] {
            check_json_string(&value, name, max).map_err(serde::de::Error::custom)?;
        }
        for name in ["signature_field", "signature_scalar"] {
            if let Some(s) = value.get(name).and_then(serde_json::Value::as_str) {
                bounded(
                    s,
                    if s.starts_with("0x") {
                        FIELD_HEX
                    } else {
                        FIELD_DECIMAL
                    },
                )
                .map_err(serde::de::Error::custom)?;
            }
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(any(feature = "p2p-webrtc", test))]
pub struct Offer(pub Box<mina_node::p2p::webrtc::Offer>);
#[cfg(any(feature = "p2p-webrtc", test))]
impl<'de> serde::Deserialize<'de> for Offer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        for (name, max) in [
            ("identity_pub_key", IDENTITY_KEY),
            ("target_peer_id", PEER_ID),
            ("chain_id", CHAIN_ID),
            ("host", HOST),
            ("sdp", SDP),
        ] {
            check_json_string(&value, name, max).map_err(serde::de::Error::custom)?;
        }
        if let Some(host) = value.get("host").and_then(serde_json::Value::as_object) {
            for s in host.values().filter_map(serde_json::Value::as_str) {
                bounded(s, HOST).map_err(serde::de::Error::custom)?;
            }
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

fn check_json_string(
    value: &serde_json::Value,
    name: &str,
    max: usize,
) -> Result<(), InputTooLong> {
    if let Some(s) = value.get(name).and_then(serde_json::Value::as_str) {
        bounded(s, max)?;
    }
    Ok(())
}

/// URL query integers arrive as strings, including arbitrarily many zeroes.
pub fn optional_usize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    let input = <Option<String> as serde::Deserialize>::deserialize(d)?;
    input
        .map(|s| {
            bounded(&s, usize::MAX.ilog10() as usize + 1)
                .map_err(serde::de::Error::custom)?
                .parse()
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

pub fn query_usize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    optional_usize(d)?.ok_or_else(|| serde::de::Error::custom("expected an integer"))
}

#[cfg(test)]
pub(crate) mod tests;
