//! Decode precomputed blocks, the JSON form nodes publish (for example to the public
//! `mina_network_block_data` bucket), into a verifiable block header.
//!
//! Adapted from `MinaProtocol/mina-verify` (`crates/mina-verify/src/precomputed.rs`,
//! Apache-2.0).

use std::sync::Arc;

use base64::Engine;
use binprot::BinProtRead;
use mina_p2p_messages::{
    list::List,
    number::UInt64,
    v2::{
        MinaBaseProofStableV2, MinaBlockHeaderStableV2, MinaStateProtocolStateValueStableV2,
        ProtocolVersionStableV2,
    },
};

use crate::Error;

/// Decode a precomputed block (`{ "version", "data": { … } }`, or the bare block
/// object) into a block header.
///
/// Only `protocol_state` and `protocol_state_proof` are read. They are the inputs to
/// proof verification; the remaining header fields are filled with placeholders.
pub fn header_from_precomputed(json: &str) -> Result<MinaBlockHeaderStableV2, Error> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(invalid)?;
    header_from_precomputed_value(&value)
}

/// [`header_from_precomputed`] for a block already parsed into JSON, either form.
pub fn header_from_precomputed_value(
    value: &serde_json::Value,
) -> Result<MinaBlockHeaderStableV2, Error> {
    let data = value.get("data").unwrap_or(value);
    let field = |name: &str| {
        data.get(name)
            .cloned()
            .ok_or_else(|| Error::InvalidBlock(format!("missing `{name}`")))
    };

    let protocol_state: MinaStateProtocolStateValueStableV2 =
        serde_json::from_value(field("protocol_state")?).map_err(invalid)?;
    let proof_b64: String =
        serde_json::from_value(field("protocol_state_proof")?).map_err(invalid)?;
    let proof_bytes = base64::engine::general_purpose::URL_SAFE
        .decode(proof_b64.as_bytes())
        .map_err(invalid)?;
    let proof = MinaBaseProofStableV2::binprot_read(&mut &proof_bytes[..]).map_err(invalid)?;

    let zero = || UInt64::from(0u64);
    Ok(MinaBlockHeaderStableV2 {
        delta_block_chain_proof: (protocol_state.previous_state_hash.clone(), List::new()),
        current_protocol_version: ProtocolVersionStableV2 {
            transaction: zero(),
            network: zero(),
            patch: zero(),
        },
        protocol_state,
        protocol_state_proof: Arc::new(proof),
        proposed_protocol_version_opt: None,
    })
}

fn invalid(e: impl std::fmt::Display) -> Error {
    Error::InvalidBlock(e.to_string())
}
