//! A block's body against its header: the header's `body_reference` must be
//! the hash of the body. The block proof covers the header only, so this is
//! what ties the staged ledger diff to the proven protocol state.

use ledger::staged_ledger::validate_block::{block_body_hash, BlockBodyValidationError};
use mina_core::block::{prevalidate::BlockPrevalidationError, Block};

/// Checks that the hash of the block's staged ledger diff, as OCaml computes
/// it for bitswap (`Staged_ledger_diff.Body.compute_reference`), is the
/// header's `body_reference`.
///
/// OCaml checks it before applying the diff:
/// <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/mina_block/validation.ml#L484-L501>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/staged_ledger_diff/body.ml#L52-L56>
pub fn validate_body_reference(block: &Block) -> Result<(), BlockPrevalidationError> {
    let expected = &block
        .header
        .protocol_state
        .body
        .blockchain_state
        .body_reference;
    let got = block_body_hash(&block.body.staged_ledger_diff).map_err(|error| match error {
        // A cached proof's file failed its read (PC2): the body may be
        // fine, and the block is neither rejected for good nor blamed on
        // its sender.
        BlockBodyValidationError::ProofUnavailable(error) => {
            BlockPrevalidationError::BodyProofUnavailable { error }
        }
        error => BlockPrevalidationError::InvalidBodyReference {
            expected: hex::encode(&expected.0 .0),
            got: format!("{error:?}"),
        },
    })?;
    if &got != expected {
        return Err(BlockPrevalidationError::InvalidBodyReference {
            expected: hex::encode(&expected.0 .0),
            got: hex::encode(&got.0 .0),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ledger::staged_ledger::validate_block::{block_body_hash, BlockBodyValidationError};
    use mina_core::block::prevalidate::BlockPrevalidationError;
    use mina_p2p_messages::{
        binprot::BinProtRead, proof_cache::ProofStore, v2::MinaBlockBlockStableV2,
    };

    use super::validate_body_reference;

    /// A mainnet block at 563978 from live gossip: an 8-byte length, the
    /// `NewState` tag, then the block.
    const GOSSIP: &[u8] =
        include_bytes!("../../../web-verifier/tests/fixtures/mainnet-563978.gossipbin");

    fn block() -> MinaBlockBlockStableV2 {
        MinaBlockBlockStableV2::binprot_read(&mut &GOSSIP[9..]).unwrap()
    }

    #[test]
    fn live_block_body_matches_its_header() {
        let block = block();
        assert!(!block.body.staged_ledger_diff.diff.0.commands.is_empty());
        validate_body_reference(&block).unwrap();
    }

    #[test]
    fn changed_body_is_rejected() {
        let mut block = block();
        block.body.staged_ledger_diff.diff.0.commands.pop_back();
        let Err(BlockPrevalidationError::InvalidBodyReference { expected, got }) =
            validate_body_reference(&block)
        else {
            panic!("a body without its last command must not match the header");
        };
        assert_eq!(expected.len(), 64);
        assert_eq!(got.len(), 64);
        assert_ne!(expected, got);
    }

    /// BP-SAFE: a block whose cached body proofs lost their files. Its body
    /// matched its header before caching.
    fn block_with_lost_cached_proofs(name: &str) -> (mina_core::block::ArcBlockWithHash, PathBuf) {
        let block = crate::persist::test_fixtures::mainnet_blocks()
            .into_iter()
            .find(|block| block.completed_works_iter().next().is_some())
            .expect("a captured OCaml block with completed work");
        validate_body_reference(block.block.as_ref()).unwrap();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-data/proof-cache/body-reference-{name}-{}",
            std::process::id()
        ));
        let store = ProofStore::dir(&dir).unwrap();
        block.cache_proofs_in(&store).unwrap();
        let mut removed = 0;
        for file in std::fs::read_dir(&dir).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
            removed += 1;
        }
        assert!(removed > 0);
        (block, dir)
    }

    /// Hashing a body whose cached proof can't be read is an error, not a
    /// panic of the thread hashing it (the ledger worker, for a producer).
    #[test]
    fn body_hash_with_an_unreadable_cached_proof_is_an_error() {
        let (block, dir) = block_with_lost_cached_proofs("hash");
        let Err(BlockBodyValidationError::ProofUnavailable(error)) =
            block_body_hash(&block.body().staged_ledger_diff)
        else {
            panic!("an unreadable cached proof must fail the body hash");
        };
        assert!(!error.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A received block whose body can't be hashed here is not rejected for
    /// good, nor reported as a body that doesn't match its header.
    #[test]
    fn body_reference_with_an_unreadable_cached_proof_is_a_local_error() {
        let (block, dir) = block_with_lost_cached_proofs("reference");
        let error = validate_body_reference(block.block.as_ref()).unwrap_err();
        assert!(
            matches!(error, BlockPrevalidationError::BodyProofUnavailable { .. }),
            "{error:?}"
        );
        assert!(!error.is_forever_invalid());
        let update = crate::verification::VerificationUpdate::headers(
            &block,
            crate::verification::Arrival::Gossip,
            &Err(error),
        );
        let crate::verification::VerificationUpdate::Headers { delta, body, .. } = update else {
            panic!("a headers update");
        };
        use crate::verification::CheckStatus;
        assert!(matches!(
            (delta, body),
            (CheckStatus::Passed, CheckStatus::NotRun)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changed_body_reference_is_rejected() {
        let mut block = block();
        let reference = &mut block
            .header
            .protocol_state
            .body
            .blockchain_state
            .body_reference;
        reference.0 .0[0] ^= 1;
        assert!(matches!(
            validate_body_reference(&block),
            Err(BlockPrevalidationError::InvalidBodyReference { .. })
        ));
    }
}
