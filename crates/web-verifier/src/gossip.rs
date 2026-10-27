//! Decode blocks as they arrive over Mina's libp2p gossip.
//!
//! A consensus gossip payload is `[8-byte little-endian length][GossipNetMessageV2]`,
//! and a block is the `NewState` variant (binprot tag 0) carrying
//! `MinaBlockBlockStableV2 { header, body }`. Only the header is decoded: it holds
//! everything proof verification and fork choice need, and it keeps decoding
//! independent of body types that hard forks change, such as Mesa's 32-field zkApp
//! state.

use binprot::BinProtRead;
use mina_p2p_messages::v2::MinaBlockHeaderStableV2;

use crate::Error;

const LENGTH_PREFIX: usize = 8;
const NEW_STATE_TAG: u8 = 0;

/// Decode the block header from a raw consensus gossip payload.
pub fn header_from_gossip(payload: &[u8]) -> Result<MinaBlockHeaderStableV2, Error> {
    let (prefix, message) = payload
        .split_at_checked(LENGTH_PREFIX)
        .ok_or_else(|| Error::InvalidBlock("payload shorter than its length prefix".into()))?;
    let declared = u64::from_le_bytes(prefix.try_into().expect("8-byte prefix"));
    if declared != message.len() as u64 {
        return Err(Error::InvalidBlock(format!(
            "length prefix says {declared} bytes, payload has {}",
            message.len()
        )));
    }
    match message.split_first() {
        Some((&NEW_STATE_TAG, block)) => MinaBlockHeaderStableV2::binprot_read(&mut &block[..])
            .map_err(|e| Error::InvalidBlock(format!("decoding block header: {e}"))),
        Some((tag, _)) => Err(Error::InvalidBlock(format!(
            "gossip message has tag {tag}, not a block"
        ))),
        None => Err(Error::InvalidBlock("empty gossip message".into())),
    }
}
