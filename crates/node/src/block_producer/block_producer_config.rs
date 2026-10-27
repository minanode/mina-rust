use mina_p2p_messages::v2::{NonZeroCurvePoint, ProtocolVersionStableV2};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockProducerConfig {
    pub pub_key: NonZeroCurvePoint,
    pub custom_coinbase_receiver: Option<NonZeroCurvePoint>,
    pub proposed_protocol_version: Option<ProtocolVersionStableV2>,
    #[serde(
        default = "default_block_inject_delay_ms",
        deserialize_with = "deserialize_block_inject_delay_ms"
    )]
    pub block_inject_delay_ms: u64,
}

pub const fn default_block_inject_delay_ms() -> u64 {
    1_000
}

pub const fn effective_block_inject_delay_ms(delay_ms: u64) -> u64 {
    if delay_ms < default_block_inject_delay_ms() {
        default_block_inject_delay_ms()
    } else {
        delay_ms
    }
}

fn deserialize_block_inject_delay_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let delay_ms = u64::deserialize(deserializer)?;
    Ok(effective_block_inject_delay_ms(delay_ms))
}

impl BlockProducerConfig {
    pub fn new(pub_key: NonZeroCurvePoint) -> Self {
        Self {
            pub_key,
            custom_coinbase_receiver: None,
            proposed_protocol_version: None,
            block_inject_delay_ms: default_block_inject_delay_ms(),
        }
    }

    pub fn coinbase_receiver(&self) -> &NonZeroCurvePoint {
        self.custom_coinbase_receiver
            .as_ref()
            .unwrap_or(&self.pub_key)
    }
}

#[cfg(test)]
mod tests {
    use super::{default_block_inject_delay_ms, BlockProducerConfig};
    use crate::account::AccountSecretKey;

    #[test]
    fn deserialized_delay_keeps_the_minimum() {
        let config =
            BlockProducerConfig::new(AccountSecretKey::genesis_producer().public_key().into());
        let mut value = serde_json::to_value(config).unwrap();
        assert_eq!(default_block_inject_delay_ms(), 1_000);
        value
            .as_object_mut()
            .unwrap()
            .remove("block_inject_delay_ms");
        let missing: BlockProducerConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(missing.block_inject_delay_ms, 1_000);

        for (requested, expected) in [(0, 1_000), (999, 1_000), (1_000, 1_000), (3_000, 3_000)] {
            value["block_inject_delay_ms"] = requested.into();
            let decoded: BlockProducerConfig = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(decoded.block_inject_delay_ms, expected);
        }
    }
}
