use mina_core::block::{AppliedBlock, ArcBlockWithHash};
use mina_p2p_messages::v2::StateHash;
use serde::{Deserialize, Serialize};

/// Retired roots, oldest first. Auxiliary snapshots live on the ledger thread;
/// no historical account masks are retained.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/extensions/root_history.ml#L19-L62>
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct RootHistory(pub(crate) Vec<ArcBlockWithHash>);

impl RootHistory {
    pub fn get(&self, hash: &StateHash) -> Option<&ArcBlockWithHash> {
        self.0.iter().find(|block| block.hash() == hash)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &ArcBlockWithHash> {
        self.0.iter()
    }

    /// A disconnected bootstrap installs a new frontier, with empty history.
    /// A multi-block root advance retires every ancestor along the old chain.
    pub fn advance(&mut self, old_chain: &[AppliedBlock], new_root: &ArcBlockWithHash) {
        let retired = old_chain
            .iter()
            .position(|block| block.hash() == new_root.hash())
            .or_else(|| {
                old_chain
                    .last()
                    .filter(|block| block.hash() == new_root.pred_hash())
                    .map(|_| old_chain.len())
            });
        let Some(retired) = retired else {
            self.0.clear();
            return;
        };
        // Do not join disconnected histories (including a restored frontier).
        if let (Some(last), Some(first)) = (self.0.last(), old_chain.first()) {
            if last.hash() != first.pred_hash() {
                self.0.clear();
            }
        }
        self.0
            .extend(old_chain[..retired].iter().map(|b| b.block.clone()));
        let capacity = new_root.constants().k.as_u32() as usize + 10;
        let excess = self.0.len().saturating_sub(capacity);
        self.0.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mina_core::block::BlockWithHash;
    use mina_p2p_messages::{rpc, rpc_kernel::PayloadBinprotReader};
    use std::sync::Arc;

    #[test]
    fn history_is_bounded_contiguous_and_reset_on_bootstrap() {
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let captured = rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .data;
        assert_eq!(
            captured.header.protocol_state.body.constants.k.as_u32(),
            290
        );
        // Synthetic ancestry tests the FIFO policy, not protocol/hash correctness.
        let mut chain: Vec<AppliedBlock> = Vec::new();
        for height in 0..305 {
            let mut block = captured.clone();
            block
                .header
                .protocol_state
                .body
                .consensus_state
                .blockchain_length = height.into();
            if let Some(parent) = chain.last() {
                block.header.protocol_state.previous_state_hash = parent.hash().clone();
            }
            chain.push(AppliedBlock {
                block: BlockWithHash::try_new(Arc::new(block)).unwrap(),
                just_emitted_a_proof: false,
            });
        }
        let mut history = RootHistory::default();
        history.advance(&chain[..301], &chain[300].block);
        assert_eq!(history.iter().count(), 300); // OCaml k + 10.
        assert_eq!(history.iter().next().unwrap().hash(), chain[0].hash());
        history.advance(&chain[300..], &chain[304].block);
        assert_eq!(history.iter().count(), 300);
        assert_eq!(history.iter().next().unwrap().hash(), chain[4].hash());
        assert_eq!(history.iter().last().unwrap().hash(), chain[303].hash());
        for retired in &chain[..4] {
            assert!(history.get(retired.hash()).is_none());
        }
        // Same root (including a best-tip reorg) adds nothing.
        history.advance(&chain[304..], &chain[304].block);
        assert_eq!(history.iter().count(), 300);
        // A disconnected rebootstrap starts with no history.
        history.advance(&chain[304..], &chain[0].block);
        assert_eq!(history.iter().count(), 0);
    }
}
