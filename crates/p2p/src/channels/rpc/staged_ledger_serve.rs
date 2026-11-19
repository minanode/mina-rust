//! Encode the libp2p staged-ledger reply without cloning its shared payload.

use std::io::{self, Write};

use binprot::BinProtWrite;
use mina_p2p_messages::rpc_kernel::RpcResultKind;

use super::StagedLedgerAuxAndPendingCoinbases;

/// The tuple returned by the OCaml RPC, borrowing each of its four fields.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L253-L289>
struct BorrowedParts<'a>(&'a StagedLedgerAuxAndPendingCoinbases);

impl BinProtWrite for BorrowedParts<'_> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        self.0.scan_state.binprot_write(w)?;
        self.0.staged_ledger_hash.inner().0.binprot_write(w)?;
        self.0.pending_coinbase.binprot_write(w)?;
        self.0.needed_blocks.binprot_write(w)
    }
}

/// Counts the bytes written, and fails the write that takes the count past
/// `max`, so that counting stops there.
struct CountBytes {
    count: usize,
    max: usize,
    over: bool,
}

impl CountBytes {
    fn new(max: usize) -> Self {
        Self {
            count: 0,
            max,
            over: false,
        }
    }
}

impl Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.count.checked_add(bytes.len()) {
            Some(count) if count <= self.max => {
                self.count = count;
                Ok(bytes.len())
            }
            _ => {
                self.over = true;
                Err(io::Error::other("staged-ledger reply over its limit"))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The length of `payload`, or `None` if it is over `max`. The pass retains no
/// bytes, and it propagates cache-read failures instead of using the
/// infallible `BinProtSize` adapter.
fn payload_len(payload: &impl BinProtWrite, max: usize) -> io::Result<Option<usize>> {
    let mut count = CountBytes::new(max);
    match payload.binprot_write(&mut count) {
        Ok(()) => Ok(Some(count.count)),
        Err(_) if count.over => Ok(None),
        Err(error) => Err(error),
    }
}

/// The framing of `RpcResult(Ok(NeedsLength(payload)))` before a payload of
/// `len` bytes: one result tag and at most nine bytes for bin_prot's Nat0
/// length.
fn result_header(len: usize) -> io::Result<([u8; 10], usize)> {
    let mut header = [0u8; 10];
    let mut cursor = io::Cursor::new(header.as_mut_slice());
    RpcResultKind::Ok.binprot_write(&mut cursor)?;
    binprot::Nat0(len as u64).binprot_write(&mut cursor)?;
    let header_len = cursor.position() as usize;
    Ok((header, header_len))
}

/// Same framing as `RpcResult(Ok(NeedsLength(payload)))`, with one output
/// allocation. The payload must encode identically on both passes. Our
/// borrowed, immutable wire parts satisfy this, including proofs whose cache
/// representation can change while their encoded bytes stay identical.
#[cfg(any(test, feature = "p2p-libp2p"))]
fn encode_success(payload: &impl BinProtWrite) -> io::Result<Vec<u8>> {
    let len = payload_len(payload, usize::MAX)?
        .ok_or_else(|| io::Error::other("staged-ledger reply length overflow"))?;
    let (header, header_len) = result_header(len)?;
    let total_len = len
        .checked_add(header_len)
        .ok_or_else(|| io::Error::other("staged-ledger reply length overflow"))?;
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(&header[..header_len]);
    payload.binprot_write(&mut bytes)?;
    if bytes.len() != total_len {
        return Err(io::Error::other("staged-ledger reply length changed"));
    }
    Ok(bytes)
}

#[cfg(feature = "p2p-libp2p")]
pub(super) fn encode(parts: &StagedLedgerAuxAndPendingCoinbases) -> io::Result<Vec<u8>> {
    encode_success(&Some(BorrowedParts(parts)))
}

/// A staged-ledger answer measured for one RPC message: `prefix` (the RPC
/// length and message header), then the reply.
pub struct StagedLedgerMessage {
    prefix: Vec<u8>,
    /// The whole message's length, `prefix` included.
    pub len: usize,
}

/// Measures the RPC message that answers with `parts`, whose message header is
/// `message_header`, or returns `None` if it is longer than `max` bytes. The
/// measuring stops at `max`, so a longer answer costs no more to refuse than
/// one at `max`. Nothing is allocated for the answer.
pub fn measure(
    parts: &StagedLedgerAuxAndPendingCoinbases,
    message_header: &[u8],
    max: usize,
) -> io::Result<Option<StagedLedgerMessage>> {
    const LENGTH: usize = 8;
    let payload = Some(BorrowedParts(parts));
    // A payload that doesn't fit beside the length and the message header
    // can't fit with the result header either.
    let room = max.saturating_sub(LENGTH + message_header.len());
    let Some(payload_len) = payload_len(&payload, room)? else {
        return Ok(None);
    };
    let (result, result_len) = result_header(payload_len)?;
    let len = LENGTH + message_header.len() + result_len + payload_len;
    if len > max {
        return Ok(None);
    }
    let mut prefix = Vec::with_capacity(LENGTH + message_header.len() + result_len);
    prefix.extend_from_slice(&((len - LENGTH) as u64).to_le_bytes());
    prefix.extend_from_slice(message_header);
    prefix.extend_from_slice(&result[..result_len]);
    Ok(Some(StagedLedgerMessage { prefix, len }))
}

/// Writes a message of a known length into chunks of `chunk` bytes, the last
/// one shorter, each allocated once at its final size.
struct Chunks {
    chunks: Vec<Vec<u8>>,
    current: Vec<u8>,
    /// The length `current` is filled to.
    target: usize,
    chunk: usize,
    /// Bytes not yet written, `current`'s included.
    remaining: usize,
}

impl Chunks {
    fn new(len: usize, chunk: usize) -> Self {
        let target = chunk.min(len);
        Self {
            chunks: Vec::with_capacity(len.div_ceil(chunk)),
            current: Vec::with_capacity(target),
            target,
            chunk,
            remaining: len,
        }
    }

    fn finish(mut self) -> io::Result<Vec<Vec<u8>>> {
        if self.current.len() != self.remaining {
            return Err(io::Error::other("staged-ledger reply length changed"));
        }
        if !self.current.is_empty() {
            self.chunks.push(self.current);
        }
        Ok(self.chunks)
    }
}

impl Write for Chunks {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let written = bytes.len();
        while !bytes.is_empty() {
            if self.current.len() == self.target {
                if self.current.len() == self.remaining {
                    return Err(io::Error::other("staged-ledger reply length changed"));
                }
                self.remaining -= self.current.len();
                self.target = self.chunk.min(self.remaining);
                let next = Vec::with_capacity(self.target);
                self.chunks.push(std::mem::replace(&mut self.current, next));
            }
            let n = (self.target - self.current.len()).min(bytes.len());
            self.current.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encodes the measured message in chunks of at most `chunk` bytes, so that
/// the answer is never held twice: the Yamux queue keeps the chunks as they
/// are, and frees each one once it is sent. Fails if the parts encode to
/// another length than measured.
pub fn encode_chunks(
    parts: &StagedLedgerAuxAndPendingCoinbases,
    message: &StagedLedgerMessage,
    chunk: usize,
) -> io::Result<Vec<Vec<u8>>> {
    let mut out = Chunks::new(message.len, chunk.max(1));
    out.write_all(&message.prefix)?;
    Some(BorrowedParts(parts)).binprot_write(&mut out)?;
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use binprot::BinProtRead;
    use mina_p2p_messages::{
        rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response,
        rpc_kernel::{NeedsLength, ResponsePayload, RpcResult},
        v2,
    };
    use std::sync::Arc;

    // This reference deliberately retains the old nested-buffer encoder.
    fn old_encoder(parts: &StagedLedgerAuxAndPendingCoinbases) -> Vec<u8> {
        let StagedLedgerAuxAndPendingCoinbases {
            scan_state,
            staged_ledger_hash,
            pending_coinbase,
            needed_blocks,
        } = parts.clone();
        let response: ResponsePayload<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response> =
            RpcResult(Ok(NeedsLength(Some((
                scan_state,
                staged_ledger_hash.inner().0.clone(),
                pending_coinbase,
                needed_blocks,
            )))));
        let mut bytes = Vec::new();
        response.binprot_write(&mut bytes).unwrap();
        bytes
    }

    #[test]
    #[ignore = "requires MINA_STAGED_LEDGER_SERVE_FIXTURE: raw OCaml v3 RPC response payload"]
    fn staged_ledger_reply_matches_ocaml_and_old_encoder() {
        let path = std::env::var_os("MINA_STAGED_LEDGER_SERVE_FIXTURE").unwrap();
        let expected = std::fs::read(path).unwrap();
        let mut reader = expected.as_slice();
        let response =
            ResponsePayload::<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>::binprot_read(
                &mut reader,
            )
            .unwrap();
        assert!(reader.is_empty());
        let (scan_state, hash, pending_coinbase, needed_blocks) =
            response.0.unwrap().0.expect("a populated OCaml scan state");
        let parts = Arc::new(StagedLedgerAuxAndPendingCoinbases {
            scan_state,
            staged_ledger_hash: v2::MinaBaseLedgerHash0StableV1(hash).into(),
            pending_coinbase,
            needed_blocks,
        });
        let old = old_encoder(&parts);
        assert!(old == expected, "old encoder differs from OCaml capture");
        let (header, actual) = super::super::libp2p::internal_response_into_libp2p(
            super::super::P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(parts.clone()),
            42,
        )
        .unwrap();
        assert_eq!(header.id, 42);
        assert!(
            actual.as_ref() == expected.as_slice(),
            "borrowed encoder differs from OCaml capture"
        );
        assert!(
            actual.as_ref() == old.as_slice(),
            "borrowed encoder differs from old encoder"
        );
        assert_eq!(Arc::strong_count(&parts), 1);
        // A still-shared response can be served repeatedly without consuming it.
        assert!(encode(&parts).unwrap() == expected);
        println!("OCaml and old encoder match: {} bytes", expected.len());
    }

    struct Raw(Vec<u8>);
    impl BinProtWrite for Raw {
        fn binprot_write<W: Write>(&self, w: &mut W) -> io::Result<()> {
            w.write_all(&self.0)
        }
    }

    #[test]
    fn framing_matches_current_encoder_at_nat0_boundaries() {
        for size in [0, 1, 127, 128, 255, 256, 65535, 65536, 131072] {
            let payload = Raw(vec![0x5a; size]);
            let actual = encode_success(&payload).unwrap();
            let response: ResponsePayload<_> = RpcResult(Ok(NeedsLength(payload)));
            let mut expected = Vec::new();
            response.binprot_write(&mut expected).unwrap();
            assert_eq!(actual, expected, "payload length {size}");
            assert_eq!(actual.capacity(), actual.len(), "no spare output capacity");
        }
    }

    /// A small staged ledger, with `blocks` copies of a Mesa protocol state
    /// as its needed blocks to give it some length.
    pub(crate) fn staged_ledger(blocks: usize) -> StagedLedgerAuxAndPendingCoinbases {
        use mina_p2p_messages::{list::List, rpc::GetBestTipV2Response};
        let bytes = include_bytes!("../../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let tip = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..])
            .unwrap()
            .0
            .unwrap()
            .0
            .unwrap();
        let state = tip.data.header.protocol_state.clone();
        StagedLedgerAuxAndPendingCoinbases {
            scan_state: v2::TransactionSnarkScanStateStableV2 {
                scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                    trees: (
                        v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![(
                            v2::ParallelScanWeightStableV1 {
                                base: 1.into(),
                                merge: 0.into(),
                            },
                            v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
                        )]),
                        List::new(),
                    ),
                    acc: None,
                    curr_job_seq_no: 0.into(),
                    max_base_jobs: 1.into(),
                    delay: 2.into(),
                },
                previous_incomplete_zkapp_updates: (
                    List::new(),
                    v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false),
                ),
            },
            staged_ledger_hash: v2::LedgerHash::zero(),
            pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                    indexes: List::new(),
                    depth: 5.into(),
                    tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(
                        v2::MinaBasePendingCoinbaseHashVersionedStableV1(
                            v2::MinaBasePendingCoinbaseHashBuilderStableV1(
                                mina_p2p_messages::bigint::BigInt::zero(),
                            ),
                        )
                        .into(),
                    ),
                },
                pos_list: List::new(),
                new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
            },
            needed_blocks: std::iter::repeat_n(state, blocks).collect(),
        }
    }

    /// The RPC message the answer was before, built by the old encoder and
    /// the generic RPC framing.
    fn old_message(parts: &StagedLedgerAuxAndPendingCoinbases, id: u64) -> Vec<u8> {
        use crate::RpcMessage;
        use mina_p2p_messages::rpc_kernel::ResponseHeader;
        RpcMessage::Response {
            header: ResponseHeader { id },
            bytes: old_encoder(parts).into(),
        }
        .into_bytes()
    }

    fn message_header(id: u64) -> Vec<u8> {
        use mina_p2p_messages::rpc_kernel::{MessageHeader, ResponseHeader};
        let mut header = Vec::new();
        MessageHeader::Response(ResponseHeader { id })
            .binprot_write(&mut header)
            .unwrap();
        header
    }

    /// The chunks are the old message, cut into pieces of the chunk size, the
    /// last one shorter, each with no spare capacity.
    #[test]
    fn chunks_are_the_old_message_in_pieces() {
        let parts = staged_ledger(40);
        for id in [0, 1, 300, u64::MAX] {
            let expected = old_message(&parts, id);
            let message = measure(&parts, &message_header(id), usize::MAX)
                .unwrap()
                .unwrap();
            assert_eq!(message.len, expected.len());
            for chunk in [1, 7, 4096, expected.len() - 1, expected.len(), 1 << 30] {
                let chunks = encode_chunks(&parts, &message, chunk).unwrap();
                assert_eq!(
                    chunks.len(),
                    expected.len().div_ceil(chunk),
                    "chunk {chunk}"
                );
                assert!(chunks.iter().all(|c| c.len() == c.capacity()));
                assert!(chunks[..chunks.len() - 1].iter().all(|c| c.len() == chunk));
                assert_eq!(chunks.concat(), expected, "chunk {chunk}, id {id}");
            }
        }
    }

    /// An answer is measured against its limit with its framing: it passes at
    /// its length and is refused one byte below.
    #[test]
    fn the_limit_counts_the_whole_message() {
        let parts = staged_ledger(40);
        let header = message_header(7);
        let len = old_message(&parts, 7).len();
        assert_eq!(measure(&parts, &header, len).unwrap().unwrap().len, len);
        assert!(measure(&parts, &header, len - 1).unwrap().is_none());
        assert!(measure(&parts, &header, 0).unwrap().is_none());
    }

    /// Measuring an answer over the limit stops at the limit.
    #[test]
    fn measuring_stops_at_the_limit() {
        struct Endless(Cell<usize>);
        impl BinProtWrite for Endless {
            fn binprot_write<W: Write>(&self, w: &mut W) -> io::Result<()> {
                loop {
                    w.write_all(&[0; 1024])?;
                    self.0.set(self.0.get() + 1024);
                }
            }
        }
        use std::cell::Cell;
        let endless = Endless(Cell::new(0));
        assert!(payload_len(&endless, 10 * 1024).unwrap().is_none());
        assert_eq!(endless.0.get(), 10 * 1024);
    }

    /// Parts that encode to another length than measured are not sent.
    #[test]
    fn a_length_that_changes_fails() {
        let parts = staged_ledger(2);
        let header = message_header(7);
        let message = measure(&parts, &header, usize::MAX).unwrap().unwrap();
        let longer = staged_ledger(3);
        assert!(encode_chunks(&longer, &message, 100).is_err());
        let shorter = staged_ledger(1);
        assert!(encode_chunks(&shorter, &message, 100).is_err());
    }

    #[test]
    fn serialization_errors_are_propagated_on_either_pass() {
        use std::cell::Cell;
        struct FailsOnPass {
            pass: Cell<usize>,
            fail_on: usize,
        }
        impl BinProtWrite for FailsOnPass {
            fn binprot_write<W: Write>(&self, w: &mut W) -> io::Result<()> {
                self.pass.set(self.pass.get() + 1);
                w.write_all(&[1, 2, 3])?;
                if self.pass.get() == self.fail_on {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "proof unavailable"));
                }
                Ok(())
            }
        }
        for fail_on in [1, 2] {
            assert_eq!(
                encode_success(&FailsOnPass {
                    pass: Cell::new(0),
                    fail_on
                })
                .unwrap_err()
                .kind(),
                io::ErrorKind::NotFound
            );
        }
    }
}
