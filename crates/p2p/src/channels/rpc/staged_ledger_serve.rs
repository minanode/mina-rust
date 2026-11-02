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

#[derive(Default)]
struct CountBytes(usize);

impl Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("staged-ledger reply length overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Same framing as `RpcResult(Ok(NeedsLength(payload)))`, with one output
/// allocation. The size pass retains no bytes; it also propagates cache-read
/// failures instead of using the infallible `BinProtSize` adapter. The payload
/// must encode identically on both passes. Our borrowed, immutable wire parts
/// satisfy this, including proofs whose cache representation can change while
/// their encoded bytes stay identical.
fn encode_success(payload: &impl BinProtWrite) -> io::Result<Vec<u8>> {
    let mut count = CountBytes::default();
    payload.binprot_write(&mut count)?;

    // One result tag and at most nine bytes for bin_prot's Nat0 length.
    let mut header = [0u8; 10];
    let mut cursor = io::Cursor::new(header.as_mut_slice());
    RpcResultKind::Ok.binprot_write(&mut cursor)?;
    binprot::Nat0(count.0 as u64).binprot_write(&mut cursor)?;
    let header_len = cursor.position() as usize;
    let total_len = count
        .0
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

pub(super) fn encode(parts: &StagedLedgerAuxAndPendingCoinbases) -> io::Result<Vec<u8>> {
    encode_success(&Some(BorrowedParts(parts)))
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
