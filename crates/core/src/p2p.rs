/// TODO: These types and methods should be moved to `p2p` crate, they are here because they are used in `snark` crates callbacks
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Copy)]
pub struct P2pNetworkPubsubMessageCacheId {
    pub source: libp2p_identity::PeerId,
    pub seqno: u64,
    /// SHA-256 of the whole encoded message. Two messages under one source
    /// and sequence number get distinct IDs, so neither replaces the other in
    /// the cache, and a verdict on one never acts on the other. It is not part
    /// of the wire ID, `to_raw_bytes`.
    pub digest: [u8; 32],
}

impl P2pNetworkPubsubMessageCacheId {
    pub fn to_raw_bytes(&self) -> Vec<u8> {
        let mut message_id = self.source.to_base58();
        message_id.push_str(&self.seqno.to_string());
        message_id.into_bytes()
    }
}
