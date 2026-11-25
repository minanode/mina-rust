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

/// The most pubsub messages queued for verification. The queue drains one
/// message a tick, so it fills only while verification is held, as during
/// block production; a message refused here comes again if its sender
/// rebroadcasts it.
pub const MAX_QUEUED_MESSAGES: usize = 256;

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
    /// Queued pubsub messages in arrival order, by arrival number: each with
    /// its ID, the peer that forwarded it, and its own commands in wire order.
    /// Arrival order keeps a sender's commands in nonce order across messages.
    /// A message is verified whole, from these exact bodies, and leaves the
    /// queue when its verification is dispatched. It is kept apart from the
    /// peer candidates in `by_peer`: a transaction hash leaves out signatures
    /// and proofs, so another copy under the same hash is not the message's
    /// own command, and the verdict on a message forwards it.
    pubsub_queue: BTreeMap<u64, QueuedMessage>,
    /// The arrival number of each queued message, by its ID.
    pubsub_by_id: BTreeMap<P2pNetworkPubsubMessageCacheId, u64>,
    pubsub_next: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct QueuedMessage {
    id: P2pNetworkPubsubMessageCacheId,
    peer_id: PeerId,
    commands: Vec<TransactionWithHash>,
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

    /// Candidates from peers, and the commands of queued pubsub messages.
    pub fn transactions_count(&self) -> usize {
        self.by_hash.len()
            + self
                .pubsub_queue
                .values()
                .map(|queued| queued.commands.len())
                .sum::<usize>()
    }

    pub fn peer_transaction_count(&self, peer_id: &PeerId) -> usize {
        self.by_peer.get(peer_id).map(|v| v.len()).unwrap_or(0)
    }

    pub fn contains(&self, hash: &TransactionHash) -> bool {
        self.by_hash.contains_key(hash)
    }

    pub fn message_id_contains(&self, message_id: &P2pNetworkPubsubMessageCacheId) -> bool {
        self.pubsub_by_id.contains_key(message_id)
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

    /// Queues a pubsub message, unless it holds no commands or the queue is
    /// full. A message with the same commands as a queued one takes its ID and
    /// relay, and keeps its place: the verdict on a message forwards it from
    /// the gossip cache, so the newer copy, likelier to still be cached, is
    /// the one to name, and the place keeps the commands ahead of any that
    /// came after them. Returns whether it was queued.
    pub fn transactions_received(
        &mut self,
        _time: Timestamp,
        peer_id: PeerId,
        transactions: Vec<TransactionWithHash>,
        message_id: P2pNetworkPubsubMessageCacheId,
    ) -> bool {
        if transactions.is_empty() {
            return false;
        }
        let same_commands = self.pubsub_queue.iter_mut().find(|(_, queued)| {
            queued.commands.len() == transactions.len()
                && queued
                    .commands
                    .iter()
                    .zip(&transactions)
                    .all(|(a, b)| a.body() == b.body())
        });
        if let Some((&place, queued)) = same_commands {
            self.pubsub_by_id.remove(&queued.id);
            queued.id = message_id;
            queued.peer_id = peer_id;
            self.pubsub_by_id.insert(message_id, place);
            return true;
        }
        if self.pubsub_queue.len() >= MAX_QUEUED_MESSAGES {
            return false;
        }
        let place = self.pubsub_next;
        self.pubsub_next += 1;
        self.pubsub_queue.insert(
            place,
            QueuedMessage {
                id: message_id,
                peer_id,
                commands: transactions,
            },
        );
        self.pubsub_by_id.insert(message_id, place);
        true
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
                            .collect::<Vec<_>>();
                        // A peer with nothing received to verify yields no
                        // batch: an empty batch is never verified, and would
                        // stay first.
                        if transactions.is_empty() {
                            continue;
                        }
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

    /// The first queued pubsub message to arrive, whole: its own commands, in
    /// its own order, so that one sender's commands reach the pool in nonce
    /// order.
    fn next_batch_from_pubsub(&self) -> Option<NextBatch> {
        let (_, queued) = self.pubsub_queue.first_key_value()?;
        Some((
            queued.peer_id,
            queued.commands.clone(),
            TransactionPoolMessageSource::pubsub(queued.id),
        ))
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
                if let Some(place) = self.pubsub_by_id.remove(id) {
                    self.pubsub_queue.remove(&place);
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
        })
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
mod pubsub_queue_tests {
    use redux::Timestamp;

    use super::*;
    use crate::{p2p::identity::SecretKey, transaction_pool::submission_tests::fixture_block};

    /// The first `n` signed payments of the fixture block, all distinct.
    fn signed(n: usize) -> Vec<TransactionWithHash> {
        let diff = fixture_block().body.staged_ledger_diff.diff;
        let signed = diff
            .0
            .commands
            .into_iter()
            .chain(diff.1.into_iter().flat_map(|d| d.commands))
            .map(|c| c.data)
            .filter(|c| matches!(c, Transaction::SignedCommand(_)))
            .map(|c| TransactionWithHash::try_new(c).unwrap())
            .take(n)
            .collect::<Vec<_>>();
        assert_eq!(signed.len(), n, "the fixture holds {n} signed commands");
        signed
    }

    fn message(seqno: u64) -> P2pNetworkPubsubMessageCacheId {
        let origin = SecretKey::from_bytes([9; 32]).public_key().peer_id();
        P2pNetworkPubsubMessageCacheId {
            source: origin.try_into().unwrap(),
            seqno,
            digest: [0; 32],
        }
    }

    fn peer(byte: u8) -> PeerId {
        SecretKey::from_bytes([byte; 32]).public_key().peer_id()
    }

    fn bodies(batch: &[TransactionWithHash]) -> Vec<Transaction> {
        batch.iter().map(|tx| tx.body().clone()).collect()
    }

    /// Runs `VerifyNext`, `VerifyPending` and `VerifySuccess` on the state.
    fn verify_next(state: &mut TransactionPoolCandidatesState) -> NextBatch {
        let (from, batch, source) = state.get_batch_to_verify().expect("a batch");
        let hashes = batch.iter().map(|tx| tx.hash().clone()).collect::<Vec<_>>();
        state.verify_pending(Timestamp::ZERO, &from, (), &hashes);
        state.verify_result(Timestamp::ZERO, &from, (), &source, Ok(()));
        (from, batch, source)
    }

    /// The same payment in two messages, as when its sender rebroadcasts it.
    /// A copy with the same commands as a queued message takes its place; one
    /// arriving after the first left the queue is queued, and verified whole.
    /// Neither leaves anything behind that could stop later messages.
    #[test]
    fn a_rebroadcast_payment_does_not_block_later_gossip() {
        let txs = signed(2);
        let (payment, later) = (txs[0].clone(), txs[1].clone());
        let mut state = TransactionPoolCandidatesState::new();
        assert!(state.transactions_received(
            Timestamp::ZERO,
            peer(1),
            vec![payment.clone()],
            message(1)
        ));
        // The same commands again: the newer message takes the queued one's
        // place, so its verdict forwards the copy likelier to be cached.
        assert!(state.transactions_received(
            Timestamp::ZERO,
            peer(2),
            vec![payment.clone()],
            message(2)
        ));

        let (from, batch, source) = verify_next(&mut state);
        assert_eq!(from, peer(2));
        assert_eq!(bodies(&batch), bodies(&[payment.clone()]));
        assert!(matches!(source, TransactionPoolMessageSource::Pubsub { id } if id == message(2)));
        assert!(state.transactions_received(
            Timestamp::ZERO,
            peer(2),
            vec![payment.clone()],
            message(3)
        ));
        assert!(state.transactions_received(
            Timestamp::ZERO,
            peer(1),
            vec![later.clone()],
            message(4)
        ));
        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&[payment]));
        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&[later]));
        assert!(state.get_batch_to_verify().is_none());
    }

    /// The queue holds at most `MAX_QUEUED_MESSAGES` messages.
    #[test]
    fn the_message_queue_is_bounded() {
        let txs = signed(1);
        let mut state = TransactionPoolCandidatesState::new();
        for seqno in 0..MAX_QUEUED_MESSAGES as u64 {
            // Distinct command lists: the payment repeated `seqno + 1` times.
            let commands = vec![txs[0].clone(); seqno as usize + 1];
            assert!(state.transactions_received(
                Timestamp::ZERO,
                peer(1),
                commands,
                message(seqno)
            ));
        }
        assert!(!state.transactions_received(
            Timestamp::ZERO,
            peer(1),
            vec![txs[0].clone(); MAX_QUEUED_MESSAGES + 1],
            message(MAX_QUEUED_MESSAGES as u64),
        ));
    }

    /// A message whose forwarding peer left is still verified under its own
    /// pubsub source.
    #[test]
    fn a_departed_peers_message_keeps_its_source() {
        let txs = signed(1);
        let mut state = TransactionPoolCandidatesState::new();
        state.transactions_received(Timestamp::ZERO, peer(1), txs.clone(), message(1));
        state.peer_remove(peer(1));

        let (from, batch, source) = verify_next(&mut state);
        assert_eq!(from, peer(1));
        assert_eq!(bodies(&batch), bodies(&txs));
        assert!(matches!(source, TransactionPoolMessageSource::Pubsub { id } if id == message(1)));
    }

    /// One sender's commands in a message reach the pool in the message's
    /// order, their nonce order; hash order rejected the later nonce.
    #[test]
    fn a_message_is_verified_in_its_own_order() {
        let mut txs = signed(2);
        txs.sort_by(|a, b| b.hash().cmp(a.hash()));
        let mut state = TransactionPoolCandidatesState::new();
        state.transactions_received(Timestamp::ZERO, peer(1), txs.clone(), message(1));

        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&txs));
    }

    /// Two copies of a command can share a hash and differ in their
    /// signature, which the hash leaves out. Each message is verified with its
    /// own copy, and neither copy replaces or removes the other.
    #[test]
    fn each_message_is_verified_with_its_own_copy() {
        let txs = signed(2);
        let good = txs[0].clone();
        let Transaction::SignedCommand(mut forged) = good.body().clone() else {
            unreachable!("signed() returns signed commands")
        };
        let Transaction::SignedCommand(other) = txs[1].body() else {
            unreachable!("signed() returns signed commands")
        };
        forged.signature = other.signature.clone();
        let forged = TransactionWithHash::try_new(Transaction::SignedCommand(forged)).unwrap();
        assert_eq!(forged.hash(), good.hash());
        assert_ne!(forged.body(), good.body());
        let mut state = TransactionPoolCandidatesState::new();

        // One relay sends both copies.
        state.transactions_received(Timestamp::ZERO, peer(1), vec![forged.clone()], message(1));
        state.transactions_received(Timestamp::ZERO, peer(1), vec![good.clone()], message(2));

        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&[forged]));
        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&[good]), "the valid copy was lost");
        assert!(state.get_batch_to_verify().is_none());
    }

    /// A gossip message without commands is not queued.
    #[test]
    fn an_empty_message_is_not_queued() {
        let mut state = TransactionPoolCandidatesState::new();
        state.transactions_received(Timestamp::ZERO, peer(1), vec![], message(1));
        assert!(state.get_batch_to_verify().is_none());
        assert_eq!(state.transactions_count(), 0);
    }

    /// The queue keeps arrival order. One publisher sends A, then B at the
    /// next nonce, then A again: A is still verified first, under its newer
    /// ID, so B does not reach the pool ahead of it and fail as a nonce gap.
    /// A message from a publisher whose ID sorts first still waits its turn.
    #[test]
    fn the_queue_keeps_arrival_order() {
        let txs = signed(2);
        let (a, b) = (vec![txs[0].clone()], vec![txs[1].clone()]);
        let mut state = TransactionPoolCandidatesState::new();
        let other = |seqno| P2pNetworkPubsubMessageCacheId {
            source: SecretKey::from_bytes([1; 32])
                .public_key()
                .peer_id()
                .try_into()
                .unwrap(),
            seqno,
            digest: [0; 32],
        };
        assert!(state.transactions_received(Timestamp::ZERO, peer(1), a.clone(), message(5)));
        assert!(state.transactions_received(Timestamp::ZERO, peer(1), b.clone(), message(6)));
        assert!(state.transactions_received(Timestamp::ZERO, peer(2), a.clone(), message(7)));
        let late = signed(3)[2..].to_vec();
        assert!(state.transactions_received(Timestamp::ZERO, peer(3), late.clone(), other(0)));

        let (from, batch, source) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&a));
        assert_eq!(from, peer(2));
        assert!(matches!(source, TransactionPoolMessageSource::Pubsub { id } if id == message(7)));
        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&b));
        let (_, batch, _) = verify_next(&mut state);
        assert_eq!(bodies(&batch), bodies(&late));
        assert!(state.get_batch_to_verify().is_none());
    }
}
