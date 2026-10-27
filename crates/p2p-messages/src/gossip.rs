use crate::{number::Int64, v2};
use binprot_derive::{BinProtRead, BinProtWrite};
use derive_more::{From, TryInto};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(
    Clone, Debug, Serialize, Deserialize, BinProtRead, BinProtWrite, PartialEq, From, TryInto,
)]
#[serde(tag = "type", content = "message", rename_all = "snake_case")]
pub enum GossipNetMessageV2 {
    NewState(Arc<v2::MinaBlockBlockStableV2>),
    // The nonces are OCaml ints. Rebroadcasts set them to the time in
    // nanoseconds, which does not fit 32 bits.
    //
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/network_pool_base.ml#L277>
    SnarkPoolDiff {
        message: v2::NetworkPoolSnarkPoolDiffVersionedStableV2,
        nonce: Int64,
    },
    TransactionPoolDiff {
        message: v2::NetworkPoolTransactionPoolDiffVersionedStableV2,
        nonce: Int64,
    },
}

impl TryFrom<GossipNetMessageV2> for v2::NetworkPoolSnarkPoolDiffVersionedStableV2 {
    type Error = ();

    fn try_from(value: GossipNetMessageV2) -> Result<Self, Self::Error> {
        match value {
            GossipNetMessageV2::SnarkPoolDiff { message, .. } => Ok(message),
            _ => Err(()),
        }
    }
}

impl TryFrom<GossipNetMessageV2> for v2::NetworkPoolTransactionPoolDiffVersionedStableV2 {
    type Error = ();

    fn try_from(value: GossipNetMessageV2) -> Result<Self, Self::Error> {
        match value {
            GossipNetMessageV2::TransactionPoolDiff { message, .. } => Ok(message),
            _ => Err(()),
        }
    }
}

/// Why a pubsub message's data was refused by [`decode_pubsub_data`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GossipDecodeError {
    #[error("data too short for the 8-byte size header: {0} bytes")]
    MissingSizeHeader(usize),
    #[error("size header claims {claimed} bytes but only {available} follow it")]
    SizeHeaderTooLarge { claimed: u64, available: usize },
    #[error("binprot error: {0}")]
    Binprot(String),
    #[error("the reader consumed {consumed} bytes but the size header says {claimed}")]
    LengthMismatch { consumed: usize, claimed: usize },
}

/// Length of the bin_prot size header in front of every gossip message.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_net2/mina_net2.ml#L136-L146>
pub const GOSSIP_SIZE_HEADER_LEN: usize = 8;

/// Decodes the data of a pubsub message exactly as OCaml's `mina_net2` does,
/// so that a message we accept, and may forward as the bytes we received, is
/// one every OCaml peer can decode. OCaml bans, for a day, the peer that sends
/// it a message it can't decode, and a node that forwards a message is the
/// sender to the next hop.
///
/// OCaml reads the data with Core's `Bigstring.read_bin_prot`: an 8-byte size
/// header (`bin_read_int_64bit`), then the message, whose reader must stop
/// exactly at the end of the header's length. Bytes after that length are
/// ignored, here as there. The message itself is read by our bin_prot
/// readers, which accept exactly what bin_prot v0.14.0 accepts for each
/// OCaml type (`libs/binprot-rs/src/int.rs`): invalid integer and tag codes
/// are refused, non-minimal integer forms are accepted. The same rule applies
/// to every message kind: transactions, snark work and blocks.
///
/// Core: `read_bin_prot_verbose_errors`, <https://github.com/janestreet/core_kernel/blob/v0.14.1/src/bigstring.ml>
pub fn decode_pubsub_data(data: &[u8]) -> Result<GossipNetMessageV2, GossipDecodeError> {
    use binprot::BinProtRead;

    if data.len() < GOSSIP_SIZE_HEADER_LEN {
        return Err(GossipDecodeError::MissingSizeHeader(data.len()));
    }
    let (header, body) = data.split_at(GOSSIP_SIZE_HEADER_LEN);
    let claimed = u64::from_le_bytes(header.try_into().expect("8 bytes"));
    let len = usize::try_from(claimed)
        .ok()
        .filter(|len| *len <= body.len())
        .ok_or(GossipDecodeError::SizeHeaderTooLarge {
            claimed,
            available: body.len(),
        })?;

    let mut reader = &body[..len];
    let message = GossipNetMessageV2::binprot_read(&mut reader)
        .map_err(|e| GossipDecodeError::Binprot(e.to_string()))?;
    if !reader.is_empty() {
        return Err(GossipDecodeError::LengthMismatch {
            consumed: len - reader.len(),
            claimed: len,
        });
    }
    Ok(message)
}
