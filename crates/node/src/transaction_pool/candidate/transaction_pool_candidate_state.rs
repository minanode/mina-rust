#![allow(clippy::unit_arg)]

use std::collections::{BTreeMap, BTreeSet};

use crate::p2p::P2pNetworkPubsubMessageCacheId;
use mina_core::transaction::TransactionPoolMessageSource;
use mina_p2p_messages::v2;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    core::transaction::{Transaction, TransactionHash, TransactionInfo, TransactionWithHash},
    p2p::{channels::rpc::P2pRpcId, PeerId},
};

static EMPTY_PEER_TX_CANDIDATES: BTreeMap<TransactionHash, TransactionPoolCandidateState> =
    BTreeMap::new();

type NextBatch = (
    PeerId,
    Vec<TransactionWithHash>,
    TransactionPoolMessageSource,
);

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TransactionPoolCandidatesState {
    by_peer: BTreeMap<PeerId, BTreeMap<TransactionHash, TransactionPoolCandidateState>>,
    by_hash: BTreeMap<TransactionHash, BTreeSet<PeerId>>,
    by_message_id: BTreeMap<P2pNetworkPubsubMessageCacheId, (PeerId, Vec<TransactionHash>)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TransactionPoolCandidateState {
    InfoReceived {
        time: Timestamp,
        info: TransactionInfo,
    },
    FetchPending {
        time: Timestamp,
        info: TransactionInfo,
        rpc_id: P2pRpcId,
    },
    Received {
        time: Timestamp,
        transaction: TransactionWithHash,
    },
    VerifyPending {
        time: Timestamp,
        transaction: TransactionWithHash,
        verify_id: (),
    },
    VerifyError {
        time: Timestamp,
        transaction: TransactionWithHash,
    },
    VerifySuccess {
        time: Timestamp,
        transaction: TransactionWithHash,
    },
}

impl TransactionPoolCandidatesState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn transactions_count(&self) -> usize {
        self.by_hash.len()
    }

    pub fn peer_transaction_count(&self, peer_id: &PeerId) -> usize {
        self.by_peer.get(peer_id).map(|v| v.len()).unwrap_or(0)
    }

    pub fn contains(&self, hash: &TransactionHash) -> bool {
        self.by_hash.contains_key(hash)
    }

    pub fn message_id_contains(&self, message_id: &P2pNetworkPubsubMessageCacheId) -> bool {
        self.by_message_id.contains_key(message_id)
    }

    pub fn peer_contains(&self, peer_id: PeerId, hash: &TransactionHash) -> bool {
        self.by_peer
            .get(&peer_id)
            .is_some_and(|txs| txs.contains_key(hash))
    }

    pub fn get(
        &self,
        peer_id: PeerId,
        hash: &TransactionHash,
    ) -> Option<&TransactionPoolCandidateState> {
        self.by_peer.get(&peer_id)?.get(hash)
    }

    fn transactions_from_peer_or_empty(
        &self,
        peer_id: PeerId,
    ) -> &BTreeMap<TransactionHash, TransactionPoolCandidateState> {
        self.by_peer
            .get(&peer_id)
            .unwrap_or(&EMPTY_PEER_TX_CANDIDATES)
    }

    pub fn candidates_from_peer_iter(
        &self,
        peer_id: PeerId,
    ) -> impl Iterator<Item = (&TransactionHash, &TransactionPoolCandidateState)> {
        self.transactions_from_peer_or_empty(peer_id).iter()
    }

    pub fn candidates_from_peer_with_hashes<'a, I>(
        &'a self,
        peer_id: PeerId,
        transaction_hashes: I,
    ) -> impl Iterator<
        Item = (
            &'a TransactionHash,
            Option<&'a TransactionPoolCandidateState>,
        ),
    >
    where
        I: IntoIterator<Item = &'a TransactionHash>,
    {
        let transactions = self.transactions_from_peer_or_empty(peer_id);
        transaction_hashes
            .into_iter()
            .map(|hash| (hash, transactions.get(hash)))
    }

    pub fn info_received(&mut self, time: Timestamp, peer_id: PeerId, info: TransactionInfo) {
        self.by_hash
            .entry(info.hash.clone())
            .or_default()
            .insert(peer_id);

        let hash = info.hash.clone();
        let state = TransactionPoolCandidateState::InfoReceived { time, info };
        self.by_peer.entry(peer_id).or_default().insert(hash, state);
    }

    pub fn peers_next_transactions_to_fetch<I, F>(
        &self,
        peers: I,
        get_order: F,
    ) -> Vec<(PeerId, TransactionHash)>
    where
        I: IntoIterator<Item = PeerId>,
        F: Copy + Fn(&TransactionHash) -> usize,
    {
        let mut needs_fetching = peers
            .into_iter()
            .filter_map(|peer_id| Some((peer_id, self.by_peer.get(&peer_id)?)))
            .flat_map(|(peer_id, transactions)| {
                transactions
                    .iter()
                    .filter(|(_, state)| {
                        matches!(state, TransactionPoolCandidateState::InfoReceived { .. })
                    })
                    .map(move |(hash, state)| (get_order(hash), state.fee(), peer_id, hash))
            })
            .collect::<Vec<_>>();
        needs_fetching
            .sort_by(|(ord1, fee1, ..), (ord2, fee2, ..)| ord1.cmp(ord2).then(fee1.cmp(fee2)));

        needs_fetching
            .into_iter()
            .scan(None, |last_ord, (ord, _, peer_id, hash)| {
                if *last_ord == Some(ord) {
                    return Some(None);
                }
                *last_ord = Some(ord);
                Some(Some((peer_id, hash.clone())))
            })
            .flatten()
            .collect()
    }

    pub fn fetch_pending(
        &mut self,
        time: Timestamp,
        peer_id: &PeerId,
        hash: &TransactionHash,
        rpc_id: P2pRpcId,
    ) {
        if let Some(state) = self
            .by_peer
            .get_mut(peer_id)
            .and_then(|transactions| transactions.get_mut(hash))
        {
            if let TransactionPoolCandidateState::InfoReceived { info, .. } = state {
                *state = TransactionPoolCandidateState::FetchPending {
                    time,
                    info: info.clone(),
                    rpc_id,
                };
            }
        }
    }

    pub fn transaction_received(
        &mut self,
        time: Timestamp,
        peer_id: PeerId,
        transaction: TransactionWithHash,
    ) {
        let hash = transaction.hash().clone();
        self.by_hash
            .entry(hash.clone())
            .or_default()
            .insert(peer_id);

        let state = TransactionPoolCandidateState::Received { time, transaction };
        self.by_peer.entry(peer_id).or_default().insert(hash, state);
    }

    pub fn transactions_received(
        &mut self,
        time: Timestamp,
        peer_id: PeerId,
        transactions: Vec<TransactionWithHash>,
        message_id: P2pNetworkPubsubMessageCacheId,
    ) {
        let transaction_hashes = transactions
            .iter()
            .map(TransactionWithHash::hash)
            .cloned()
            .collect::<Vec<_>>();

        self.by_message_id
            .insert(message_id, (peer_id, transaction_hashes));

        transactions.into_iter().for_each(|transaction| {
            self.transaction_received(time, peer_id, transaction);
        })
    }

    /// Get next batch of transactions to verify,
    /// first checks if there are any transactions to verify from pubsub
    /// after that checks for transactions from peers
    pub fn get_batch_to_verify(&self) -> Option<NextBatch> {
        self.next_batch_from_pubsub()
            .or_else(|| self.next_batch_from_peers())
    }

    fn next_batch_from_peers(&self) -> Option<NextBatch> {
        for (hash, peers) in self.by_hash.iter() {
            if let Some(res) = None.or_else(|| {
                for peer_id in peers {
                    let peer_transactions = self.by_peer.get(peer_id)?;
                    if peer_transactions.get(hash)?.transaction().is_some() {
                        let transactions = peer_transactions
                            .iter()
                            .filter_map(|(_, v)| match v {
                                TransactionPoolCandidateState::Received { transaction, .. } => {
                                    Some(transaction)
                                }
                                _ => None,
                            })
                            .cloned()
                            .collect();
                        return Some((*peer_id, transactions, TransactionPoolMessageSource::None));
                    }
                }
                None
            }) {
                return Some(res);
            }
        }

        None
    }

    /// The first pubsub message with a transaction still waiting from its
    /// peer. A message with none yields no batch: an empty batch is never
    /// verified, so it would stay first and stop every later message.
    fn next_batch_from_pubsub(&self) -> Option<NextBatch> {
        self.by_message_id
            .iter()
            .find_map(|(message_id, (peer_id, transaction_hashes))| {
                let transactions = self
                    .by_peer
                    .get(peer_id)?
                    .iter()
                    .filter_map(|(hash, state)| {
                        let TransactionPoolCandidateState::Received { transaction, .. } = state
                        else {
                            return None;
                        };
                        if transaction_hashes.contains(hash) {
                            Some(transaction)
                        } else {
                            None
                        }
                    })
                    .cloned()
                    .collect::<Vec<_>>();

                (!transactions.is_empty()).then(|| {
                    (
                        *peer_id,
                        transactions,
                        TransactionPoolMessageSource::pubsub(*message_id),
                    )
                })
            })
    }

    /// Drops the pubsub messages none of whose transactions still wait from
    /// their peer: another copy was verified first, or the peer left.
    fn prune_message_ids(&mut self) {
        let by_peer = &self.by_peer;
        self.by_message_id
            .retain(|_, (peer_id, transaction_hashes)| {
                by_peer
                    .get(peer_id)
                    .is_some_and(|txs| transaction_hashes.iter().any(|hash| txs.contains_key(hash)))
            });
    }

    pub fn verify_pending(
        &mut self,
        time: Timestamp,
        peer_id: &PeerId,
        verify_id: (),
        transaction_hashes: &[TransactionHash],
    ) {
        let Some(peer_transactions) = self.by_peer.get_mut(peer_id) else {
            return;
        };

        for hash in transaction_hashes {
            if let Some(job_state) = peer_transactions.get_mut(hash) {
                if let TransactionPoolCandidateState::Received { transaction, .. } = job_state {
                    *job_state = TransactionPoolCandidateState::VerifyPending {
                        time,
                        transaction: transaction.clone(),
                        verify_id,
                    };
                }
            }
        }
    }

    pub fn verify_result(
        &mut self,
        _time: Timestamp,
        peer_id: &PeerId,
        verify_id: (),
        from_source: &TransactionPoolMessageSource,
        _result: Result<(), ()>,
    ) {
        match from_source {
            TransactionPoolMessageSource::Pubsub { id } => {
                let Some((_, transactions)) = self.by_message_id.remove(id) else {
                    return;
                };

                for hash in transactions {
                    self.transaction_remove(&hash);
                }
            }
            _ => {
                if let Some(peer_transactions) = self.by_peer.get_mut(peer_id) {
                    let txs_to_remove = peer_transactions
                        .iter()
                        .filter(|(_, job_state)| job_state.pending_verify_id() == Some(verify_id))
                        .map(|(hash, _)| hash.clone())
                        .collect::<Vec<_>>();

                    for hash in txs_to_remove {
                        self.transaction_remove(&hash);
                    }
                }
            }
        }
        self.prune_message_ids();
    }

    pub fn peer_remove(&mut self, peer_id: PeerId) {
        if let Some(txs) = self.by_peer.remove(&peer_id) {
            for hash in txs.into_keys() {
                if let Some(peers) = self.by_hash.get_mut(&hash) {
                    peers.remove(&peer_id);
                    if peers.is_empty() {
                        self.by_hash.remove(&hash);
                    }
                }
            }
        }
        self.prune_message_ids();
    }

    pub fn peer_transaction_remove(&mut self, peer_id: PeerId, hash: &TransactionHash) {
        if let Some(txs) = self.by_peer.get_mut(&peer_id) {
            txs.remove(hash);
            if let Some(peers) = self.by_hash.get_mut(hash) {
                peers.remove(&peer_id);
                if peers.is_empty() {
                    self.by_hash.remove(hash);
                }
            }
        }
        self.prune_message_ids();
    }

    fn transaction_remove(&mut self, hash: &TransactionHash) {
        if let Some(peers) = self.by_hash.remove(hash) {
            for peer_id in peers {
                if let Some(txs) = self.by_peer.get_mut(&peer_id) {
                    txs.remove(hash);
                }
            }
        }
    }

    pub fn remove_inferior_transactions(&mut self, transaction: &Transaction) {
        // TODO(binier)
        match transaction.hash() {
            Err(err) => {
                mina_core::bug_condition!("tx hashing failed: {err}");
            }
            Ok(hash) => self.transaction_remove(&hash),
        };
        self.prune_message_ids();
    }

    pub fn retain<F1, F2>(&mut self, mut predicate: F1)
    where
        F1: FnMut(&TransactionHash) -> F2,
        F2: FnMut(&TransactionPoolCandidateState) -> bool,
    {
        let by_peer = &mut self.by_peer;
        self.by_hash.retain(|hash, peers| {
            let mut predicate = predicate(hash);
            peers.retain(|peer_id| {
                if let Some(peer_txs) = by_peer.get_mut(peer_id) {
                    match peer_txs.get(hash) {
                        Some(s) if predicate(s) => true,
                        Some(_) => {
                            peer_txs.remove(hash);
                            false
                        }
                        None => false,
                    }
                } else {
                    false
                }
            });
            !peers.is_empty()
        });
        self.prune_message_ids();
    }
}

impl TransactionPoolCandidateState {
    pub fn fee(&self) -> u64 {
        match self {
            Self::InfoReceived { info, .. } | Self::FetchPending { info, .. } => info.fee,
            Self::Received { transaction, .. }
            | Self::VerifyPending { transaction, .. }
            | Self::VerifyError { transaction, .. }
            | Self::VerifySuccess { transaction, .. } => match transaction.body() {
                v2::MinaBaseUserCommandStableV2::SignedCommand(v) => v.payload.common.fee.as_u64(),
                v2::MinaBaseUserCommandStableV2::ZkappCommand(v) => v.fee_payer.body.fee.as_u64(),
            },
        }
    }

    pub fn transaction(&self) -> Option<&TransactionWithHash> {
        match self {
            Self::InfoReceived { .. } => None,
            Self::FetchPending { .. } => None,
            Self::Received { transaction, .. } => Some(transaction),
            Self::VerifyPending { transaction, .. } => Some(transaction),
            Self::VerifyError { transaction, .. } => Some(transaction),
            Self::VerifySuccess { transaction, .. } => Some(transaction),
        }
    }

    pub fn pending_verify_id(&self) -> Option<()> {
        match self {
            Self::VerifyPending { verify_id, .. } => Some(*verify_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod stale_pubsub_entry_tests {
    use redux::Timestamp;

    use super::*;
    use crate::{p2p::identity::SecretKey, transaction_pool::submission_tests::fixture_block};

    /// The first two signed payments of the fixture block.
    fn payments() -> (TransactionWithHash, TransactionWithHash) {
        let diff = fixture_block().body.staged_ledger_diff.diff;
        let mut signed = diff
            .0
            .commands
            .into_iter()
            .chain(diff.1.into_iter().flat_map(|d| d.commands))
            .map(|c| c.data)
            .filter(|c| matches!(c, Transaction::SignedCommand(_)))
            .map(|c| TransactionWithHash::try_new(c).unwrap());
        let first = signed.next().expect("a signed command");
        let second = signed.next().expect("a second signed command");
        assert_ne!(first.hash(), second.hash());
        (first, second)
    }

    /// A payment that reaches the node in two gossip messages (its sender
    /// rebroadcasts it) must not stop the queue once one copy is verified.
    #[test]
    fn a_verified_duplicate_does_not_block_later_gossip() {
        let peer = SecretKey::from_bytes([1; 32]).public_key().peer_id();
        let id = |seqno| P2pNetworkPubsubMessageCacheId {
            source: peer.try_into().unwrap(),
            seqno,
        };
        let time = Timestamp::ZERO;
        let (payment, later) = payments();
        let mut state = TransactionPoolCandidatesState::new();

        state.transactions_received(time, peer, vec![payment.clone()], id(1));
        state.transactions_received(time, peer, vec![payment.clone()], id(2));

        // Verify the first copy, as `VerifyNext`, `VerifyPending` and
        // `VerifySuccess` do.
        let (from, batch, source) = state.get_batch_to_verify().expect("a batch");
        assert_eq!(batch.len(), 1);
        let hashes = batch.iter().map(|tx| tx.hash().clone()).collect::<Vec<_>>();
        state.verify_pending(time, &from, (), &hashes);
        state.verify_result(time, &from, (), &source, Ok(()));

        state.transactions_received(time, peer, vec![later.clone()], id(3));

        let (_, batch, _) = state.get_batch_to_verify().expect("a batch");
        assert!(
            batch.iter().any(|tx| tx.hash() == later.hash()),
            "the next batch holds {} commands, not the later payment",
            batch.len()
        );
        assert_eq!(state.by_message_id.len(), 1, "message 2 is left queued");
    }

    /// A message whose forwarding peer disconnected must not keep later
    /// messages off the pubsub path, where they get their gossip verdict.
    #[test]
    fn a_departed_peer_does_not_block_pubsub_verification() {
        let origin = SecretKey::from_bytes([3; 32]).public_key().peer_id();
        let id = |seqno| P2pNetworkPubsubMessageCacheId {
            source: origin.try_into().unwrap(),
            seqno,
        };
        let gone = SecretKey::from_bytes([1; 32]).public_key().peer_id();
        let here = SecretKey::from_bytes([2; 32]).public_key().peer_id();
        let time = Timestamp::ZERO;
        let (payment, later) = payments();
        let mut state = TransactionPoolCandidatesState::new();

        state.transactions_received(time, gone, vec![payment], id(1));
        state.peer_remove(gone);
        state.transactions_received(time, here, vec![later.clone()], id(2));

        let (from, batch, source) = state.get_batch_to_verify().expect("a batch");
        assert_eq!(from, here);
        assert!(batch.iter().any(|tx| tx.hash() == later.hash()));
        assert!(
            matches!(source, TransactionPoolMessageSource::Pubsub { .. }),
            "the later message is verified without its pubsub source: {source:?}"
        );
        assert_eq!(state.by_message_id.len(), 1, "message 1 is left queued");
    }
}
