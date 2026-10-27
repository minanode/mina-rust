//! Outbound block gossip policy. RPC catchup is deliberately outside this policy.
use std::collections::{BTreeMap, BTreeSet};

use binprot::BinProtRead;
use mina_core::constants::constraint_constants;
use mina_p2p_messages::{gossip::GossipNetMessageV2, v2};
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use super::pb;
use crate::PeerId;

/// Maximum explicit IWANT responses per peer and block during its gossip window.
/// go-libp2p-pubsub's default GossipSubGossipRetransmission is 3:
/// <https://github.com/libp2p/go-libp2p-pubsub/blob/v0.14.0/gossipsub.go#L50>
/// <https://github.com/libp2p/go-libp2p-pubsub/blob/v0.14.0/gossipsub.go#L778-L805>
pub(super) const MAX_IWANT_RESPONSES: u8 = 3;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct BlockGossipState {
    // Retain every hash until it is too old to gossip, rather than evicting
    // still-fresh hashes after a fixed number of messages or peer reconnects.
    announced: BTreeMap<v2::StateHash, u64>,
    sent: BTreeMap<v2::StateHash, (u64, BTreeSet<PeerId>)>,
    // Unlike the stream's send marks, the retry budget survives reconnects.
    // Count by state hash so alternate pubsub IDs cannot replenish it.
    #[serde(default)]
    iwant_responses: BTreeMap<v2::StateHash, (u64, BTreeMap<PeerId, u8>)>,
}

impl malloc_size_of::MallocSizeOf for BlockGossipState {
    fn size_of(&self, _: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        self.announced.len() * size_of::<(v2::StateHash, u64)>()
            + self.sent.len() * size_of::<(v2::StateHash, (u64, BTreeSet<PeerId>))>()
            + self
                .sent
                .values()
                .map(|(_, peers)| peers.len() * size_of::<PeerId>())
                .sum::<usize>()
            + self.iwant_responses.len() * size_of::<(v2::StateHash, (u64, BTreeMap<PeerId, u8>))>()
            + self
                .iwant_responses
                .values()
                .map(|(_, peers)| peers.len() * size_of::<(PeerId, u8)>())
                .sum::<usize>()
    }
}

struct BlockGossip {
    hash: v2::StateHash,
    slot: u32,
    start_ms: u64,
    end_ms: u64,
    genesis: bool,
}

/// The interval equivalent to OCaml's `0 <= received_slot - block_slot < delta + 1`.
/// Slots are since the hard fork, NOT `global_slot_since_genesis`; the runtime
/// genesis timestamp is the hard-fork origin. Checked arithmetic fails closed.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2748-L2769>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/epoch.ml#L10-L75>
fn slot_window(genesis_ms: u64, slot: u32, delta: u32, duration_ms: u64) -> Option<(u64, u64)> {
    if duration_ms == 0 {
        return None;
    }
    let start = genesis_ms.checked_add(u64::from(slot).checked_mul(duration_ms)?)?;
    // OCaml adds in UInt32 before widening to Int64.
    let width = u64::from(delta.wrapping_add(1)).checked_mul(duration_ms)?;
    Some((start, start.checked_add(width)?))
}

impl BlockGossip {
    fn from_state(state: &v2::MinaStateProtocolStateValueStableV2) -> Option<Self> {
        let body = &state.body;
        let consensus = &body.consensus_state;
        let slot = consensus
            .curr_global_slot_since_hard_fork
            .slot_number
            .as_u32();
        let (start_ms, end_ms) = slot_window(
            body.constants.genesis_state_timestamp.as_u64(),
            slot,
            body.constants.delta.as_u32(),
            constraint_constants().block_window_duration_ms,
        )?;
        let height = consensus.blockchain_length.as_u32();
        Some(Self {
            hash: state.try_hash().ok()?,
            slot,
            start_ms,
            end_ms,
            // OCaml's is_genesis_state uses slot zero since the hard fork.
            // Also keep Rust's height-based genesis guard.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2077-L2090>
            genesis: slot == 0
                || height == 1
                || constraint_constants()
                    .fork
                    .as_ref()
                    .is_some_and(|fork| fork.blockchain_length.checked_add(1) == Some(height)),
        })
    }

    fn valid_at(&self, time: Timestamp) -> bool {
        !self.genesis && self.start_ms <= time_ms(time) && time_ms(time) < self.end_ms
    }

    /// Read only the protocol state, not the proof or staged ledger body.
    /// NewState is tag 0, followed by block.header.protocol_state. The full
    /// message was encoded locally or decoded and validated on receipt.
    fn from_message(message: &pb::Message) -> Result<Option<Self>, ()> {
        let bytes = message.data.as_deref().ok_or(())?;
        match bytes.get(8) {
            Some(0) => {
                let state = v2::MinaStateProtocolStateValueStableV2::binprot_read(&mut &bytes[9..])
                    .map_err(|_| ())?;
                Self::from_state(&state).map(Some).ok_or(())
            }
            Some(1 | 2) => Ok(None),
            _ => Err(()),
        }
    }
}

fn time_ms(time: Timestamp) -> u64 {
    u64::from(time) / 1_000_000
}

impl BlockGossipState {
    pub fn message_identity(message: &pb::Message) -> Option<(v2::StateHash, u32)> {
        BlockGossip::from_message(message)
            .ok()
            .flatten()
            .map(|block| (block.hash, block.slot))
    }

    pub fn announce_rejection(
        &self,
        message: &GossipNetMessageV2,
        time: Timestamp,
    ) -> Option<&'static str> {
        let GossipNetMessageV2::NewState(block) = message else {
            return None;
        };
        let Some(block) = BlockGossip::from_state(&block.header.protocol_state) else {
            return Some("invalid_protocol_state");
        };
        if !block.valid_at(time) {
            Some("outside_slot_window")
        } else if self.announced.contains_key(&block.hash) {
            Some("already_announced")
        } else {
            None
        }
    }

    pub fn is_block(message: &pb::Message) -> bool {
        matches!(BlockGossip::from_message(message), Ok(Some(_)))
    }

    pub fn prune(&mut self, time: Timestamp) {
        let now = time_ms(time);
        self.announced.retain(|_, end| now < *end);
        self.sent.retain(|_, (end, _)| now < *end);
        self.iwant_responses.retain(|_, (end, _)| now < *end);
    }

    /// A transport handoff is not delivery. Forget send marks when its stream
    /// fails, but retain unsolicited announcement history and retry budgets.
    pub fn forget_peer(&mut self, peer: &PeerId) {
        self.sent.retain(|_, (_, peers)| {
            peers.remove(peer);
            !peers.is_empty()
        });
    }

    /// Explicit requests may retry even after a previous transport handoff.
    pub fn send_iwant(&mut self, message: &pb::Message, peer: PeerId, time: Timestamp) -> bool {
        match BlockGossip::from_message(message) {
            Ok(Some(block)) if block.valid_at(time) => {
                let (_, peers) = self
                    .iwant_responses
                    .entry(block.hash.clone())
                    .or_insert_with(|| (block.end_ms, BTreeMap::new()));
                let count = peers.entry(peer).or_default();
                if *count >= MAX_IWANT_RESPONSES {
                    mina_core::debug!(time; kind = "BlockGossipSuppressed",
                        state_hash = block.hash.to_string(), slot = block.slot,
                        reason = "iwant_retry_limit", phase = "transport",
                        peer_id = display(peer));
                    return false;
                }
                *count += 1;
                self.sent
                    .entry(block.hash)
                    .or_insert_with(|| (block.end_ms, BTreeSet::new()))
                    .1
                    .insert(peer);
                true
            }
            Ok(None) => true,
            _ => false,
        }
    }

    pub fn can_announce(&self, message: &GossipNetMessageV2, time: Timestamp) -> bool {
        self.announce_rejection(message, time).is_none()
    }

    /// Called when signing finishes or a validated message is forwarded.
    /// A second pubsub ID/encoding for the same state hash is not a new block.
    pub fn announce(&mut self, message: &pb::Message, time: Timestamp) -> bool {
        match BlockGossip::from_message(message) {
            Ok(Some(block)) => {
                self.prune(time);
                if !block.valid_at(time) || self.announced.contains_key(&block.hash) {
                    mina_core::debug!(time;
                        kind = "BlockGossipSuppressed",
                        state_hash = block.hash.to_string(), slot = block.slot,
                        reason = if block.valid_at(time) { "already_announced" } else { "outside_slot_window" },
                        phase = "signed",
                    );
                    return false;
                }
                self.announced.insert(block.hash, block.end_ms);
                true
            }
            Ok(None) => true,
            Err(()) => false,
        }
    }

    /// Age check for IHAVE and IWANT. Announcing a hash does not prevent an
    /// interested peer receiving its first copy through IWANT.
    pub fn is_fresh(message: &pb::Message, time: Timestamp) -> bool {
        match BlockGossip::from_message(message) {
            Ok(Some(block)) => block.valid_at(time),
            Ok(None) => true,
            Err(()) => false,
        }
    }

    /// Last check before handing a pubsub RPC to the transport. In particular,
    /// a formerly fresh IWANT reply may have expired while queued.
    pub fn send(&mut self, message: &pb::Message, peer: PeerId, time: Timestamp) -> bool {
        match BlockGossip::from_message(message) {
            Ok(Some(block)) => {
                if !block.valid_at(time) {
                    mina_core::debug!(time; kind = "BlockGossipSuppressed",
                        state_hash = block.hash.to_string(), slot = block.slot,
                        reason = "outside_slot_window", phase = "transport");
                    return false;
                }
                let hash = block.hash.clone();
                let slot = block.slot;
                let (_, peers) = self
                    .sent
                    .entry(block.hash)
                    .or_insert_with(|| (block.end_ms, BTreeSet::new()));
                if peers.insert(peer) {
                    true
                } else {
                    mina_core::debug!(time; kind = "BlockGossipSuppressed",
                        state_hash = hash.to_string(), slot = slot,
                        reason = "already_sent_to_peer", phase = "transport",
                        peer_id = display(peer));
                    false
                }
            }
            Ok(None) => true,
            Err(()) => false,
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub fn mesa_block() -> std::sync::Arc<v2::MinaBlockBlockStableV2> {
        use mina_p2p_messages::{list::List, rpc_kernel::ResponsePayload};
        let bytes = include_bytes!(
            "../../../../p2p-messages/tests/files/mesa/get_transition_chain_v2_563900_zkapp.bin"
        );
        let payload = ResponsePayload::<Option<List<v2::MinaBlockBlockStableV2>>>::binprot_read(
            &mut &bytes[..],
        )
        .unwrap();
        std::sync::Arc::new(payload.0.unwrap().0.unwrap().iter().next().unwrap().clone())
    }

    pub fn slot_start(block: &v2::MinaBlockBlockStableV2) -> Timestamp {
        let body = &block.header.protocol_state.body;
        // Mesa constants from OCaml, intentionally independent of slot_window.
        Timestamp::new(
            (body.constants.genesis_state_timestamp.as_u64()
                + u64::from(
                    body.consensus_state
                        .curr_global_slot_since_hard_fork
                        .slot_number
                        .as_u32(),
                ) * 90_000)
                * 1_000_000,
        )
    }

    pub fn message(block: std::sync::Arc<v2::MinaBlockBlockStableV2>, seqno: u64) -> pb::Message {
        pb::Message {
            from: Some(
                libp2p_identity::PeerId::try_from(
                    crate::identity::SecretKey::from_bytes([2; 32])
                        .public_key()
                        .peer_id(),
                )
                .unwrap()
                .to_bytes(),
            ),
            seqno: Some(seqno.to_be_bytes().to_vec()),
            data: Some(super::super::encode_message(&GossipNetMessageV2::NewState(block)).unwrap()),
            topic: "coda/consensus-messages/0.0.1".to_owned(),
            signature: Some(vec![3; 64]),
            key: None,
        }
    }

    #[test]
    fn mesa_wire_state_and_hard_fork_clock() {
        let block = mesa_block();
        let start = slot_start(&block);
        let info = BlockGossip::from_message(&message(block.clone(), 0))
            .unwrap()
            .unwrap();
        assert!(!info.valid_at(Timestamp::new(u64::from(start) - 1)));
        assert!(info.valid_at(start));
        assert!(info.valid_at(start + 89_999_999_999));
        assert!(!info.valid_at(start + 90_000_000_000));
        assert!(!info.valid_at(Timestamp::ZERO));
    }

    #[test]
    fn genesis_and_reannouncements_are_never_gossip() {
        let mut block = mesa_block();
        let time = slot_start(&block);
        let message1 = message(block.clone(), 1);
        let message2 = message(block.clone(), 2);
        let mut state = BlockGossipState::default();
        let new_state = GossipNetMessageV2::NewState(block.clone());
        assert_eq!(state.announce_rejection(&new_state, time), None);
        assert_eq!(
            state.announce_rejection(&new_state, Timestamp::new(u64::from(time) - 1)),
            Some("outside_slot_window")
        );
        assert!(state.announce(&message1, time));
        assert_eq!(
            state.announce_rejection(&new_state, time),
            Some("already_announced")
        );
        assert!(!state.announce(&message2, time));
        let p1 = PeerId::from_bytes([1; 32]);
        let p2 = PeerId::from_bytes([2; 32]);
        assert!(state.send(&message1, p1, time));
        assert!(!state.send(&message2, p1, time));
        assert!(state.send(&message2, p2, time));
        state.prune(time + 90_000_000_000);
        assert!(state.announced.is_empty() && state.sent.is_empty());
        assert!(!state.announce(&message1, time + 90_000_000_000));
        assert!(!state.send(&message1, p1, time + 90_000_000_000));
        let body = &mut std::sync::Arc::make_mut(&mut block)
            .header
            .protocol_state
            .body;
        body.consensus_state
            .curr_global_slot_since_hard_fork
            .slot_number =
            mina_p2p_messages::v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
                0.into(),
            );
        let genesis_time = slot_start(&block);
        assert_eq!(
            state.announce_rejection(&GossipNetMessageV2::NewState(block.clone()), genesis_time),
            Some("outside_slot_window")
        );
        assert!(!state.announce(&message(block, 3), genesis_time));
    }

    #[test]
    fn duplicate_guard_outlives_message_cache_capacity() {
        let block = mesa_block();
        let time = slot_start(&block);
        let original = message(block.clone(), 0);
        let mut state = BlockGossipState::default();
        assert!(state.announce(&original, time));
        // A burst of distinct validated forks in one slot must not evict a
        // still-fresh hash (mcache capacity 100, source history capacity 128).
        for i in 1..=150u64 {
            let mut fork = (*block).clone();
            let mut bytes = [0; 32];
            bytes[..8].copy_from_slice(&i.to_le_bytes());
            fork.header.protocol_state.previous_state_hash = v2::DataHashLibStateHashStableV1(
                mina_p2p_messages::bigint::BigInt::from_bytes(bytes),
            )
            .into();
            assert!(state.announce(&message(fork.into(), i), time));
        }
        assert!(!state.announce(&original, time));
    }

    #[test]
    fn malformed_blocks_fail_closed_and_pool_gossip_is_unchanged() {
        let block = mesa_block();
        let time = slot_start(&block);
        let mut msg = message(block, 1);
        let mut state = BlockGossipState::default();
        msg.data = Some(vec![0; 9]);
        assert!(!state.announce(&msg, time));
        assert!(!state.send(&msg, PeerId::from_bytes([1; 32]), time));
        // Empty Transaction_pool_diff with nonce 0, OCaml bin_prot tag 2.
        msg.data = Some(vec![3, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0]);
        assert!(state.announce(&msg, time));
        assert!(state.announce(&msg, time));
        assert!(state.send(&msg, PeerId::from_bytes([1; 32]), time));
        assert!(state.send(&msg, PeerId::from_bytes([1; 32]), time));
    }

    #[test]
    fn ocaml_slot_window_boundaries() {
        // OCaml received_within_window: Mesa delta=0, 90-second slots.
        // Absolute time values are arbitrary; expected slot differences are
        // taken from the OCaml rule, not generated by the Rust predicate.
        let (start, end) = slot_window(1_000, 7, 0, 90_000).unwrap();
        assert_eq!((start, end), (631_000, 721_000));
        for (time, accepted) in [
            (630_999, false),
            (631_000, true),
            (720_999, true),
            (721_000, false),
        ] {
            assert_eq!(start <= time && time < end, accepted);
        }
        assert_eq!(slot_window(1_000, 7, 2, 90_000), Some((631_000, 901_000)));
        // Crossing epoch 0 -> 1 (7140 slots) has no extra grace period.
        assert_eq!(
            slot_window(0, 7139, 0, 90_000),
            Some((642_510_000, 642_600_000))
        );
        assert_eq!(
            slot_window(0, 7140, 0, 90_000),
            Some((642_600_000, 642_690_000))
        );
        assert_eq!(
            slot_window(0, u32::MAX, 0, 90_000),
            Some((386_547_056_550_000, 386_547_056_640_000))
        );
        assert_eq!(
            slot_window(0, 7, u32::MAX, 90_000),
            Some((630_000, 630_000))
        );
        assert_eq!(slot_window(u64::MAX, 1, 0, 90_000), None);
        assert_eq!(slot_window(0, 1, 0, 0), None);
    }
}
