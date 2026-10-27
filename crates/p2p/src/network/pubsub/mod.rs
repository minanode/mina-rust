pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/gossipsub.rs"));
}
pub use mina_core::p2p::P2pNetworkPubsubMessageCacheId;
mod block_gossip;
mod p2p_network_pubsub_actions;
pub use self::p2p_network_pubsub_actions::P2pNetworkPubsubAction;

mod p2p_network_pubsub_state;
pub use self::p2p_network_pubsub_state::{
    P2pNetworkPubsubBlockSource, P2pNetworkPubsubBlockSources, P2pNetworkPubsubClientState,
    P2pNetworkPubsubClientTopicState, P2pNetworkPubsubState,
};

#[cfg(feature = "p2p-libp2p")]
mod p2p_network_pubsub_reducer;

#[cfg(feature = "p2p-libp2p")]
const TOPIC: &str = "coda/consensus-messages/0.0.1";

pub mod pubsub_effectful;
use mina_core::snark::SnarkJobId;
pub use pubsub_effectful::P2pNetworkPubsubEffectfulAction;

use binprot::BinProtWrite;
use mina_core::bug_condition;
use mina_p2p_messages::gossip::GossipNetMessageV2;
use sha2::{Digest, Sha256};

use crate::identity::SecretKey;

#[derive(serde::Serialize, serde:: Deserialize, Debug, Clone)]
pub enum BroadcastMessageId {
    BlockHash {
        hash: mina_p2p_messages::v2::StateHash,
    },
    Snark {
        job_id: SnarkJobId,
    },
    MessageId {
        message_id: P2pNetworkPubsubMessageCacheId,
    },
}

pub(super) fn webrtc_source_sk(message: &GossipNetMessageV2) -> SecretKey {
    let mut hasher = Sha256::new();
    if let Err(err) = message.binprot_write(&mut hasher) {
        bug_condition!("trying to broadcast message which can't be binprot serialized! err: {err}");
        return SecretKey::from_bytes([0; 32]);
    }
    SecretKey::from_bytes(hasher.finalize().into())
}

pub(super) fn webrtc_source_sk_from_bytes(bytes: &[u8]) -> SecretKey {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    SecretKey::from_bytes(hasher.finalize().into())
}

pub(super) fn encode_message(message: &GossipNetMessageV2) -> std::io::Result<Vec<u8>> {
    let mut buffer = vec![0; 8];

    message.binprot_write(&mut buffer)?;

    let len = buffer.len() - 8;
    buffer[..8].clone_from_slice(&(len as u64).to_le_bytes());

    Ok(buffer)
}

/// The original advertised statement is forwarded with gossip, so it must
/// agree with the proofs as well as the pool's requested statement.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L354-L369>
fn checked_snark_gossip(
    advertised: &mina_p2p_messages::v2::TransactionSnarkWorkStatementStableV2,
    snark: &mina_core::snark::Snark,
) -> Result<(), String> {
    if &snark.statement() == advertised {
        Ok(())
    } else {
        Err("snark gossip statement and proof do not match".into())
    }
}

/// Recheck every actual publication (including IWANT replies) and announcement.
/// A message that was eligible five minutes ago need not be eligible now.
fn filter_snark_gossip(
    rpc: &mut pb::Rpc,
    cache: &p2p_network_pubsub_state::P2pNetworkPubsubMessageCache,
    eligible: impl Fn(&mina_core::snark::Snark) -> bool,
) {
    use binprot::BinProtRead;
    use mina_p2p_messages::v2::NetworkPoolSnarkPoolDiffVersionedStableV2;
    let allowed = |message: &pb::Message| {
        let Some(bytes) = message.data.as_deref() else {
            return false;
        };
        match bytes.get(8) {
            Some(0 | 2) => true,
            Some(1) => {
                let Ok(GossipNetMessageV2::SnarkPoolDiff {
                    message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(work),
                    ..
                }) = GossipNetMessageV2::binprot_read(&mut &bytes[8..])
                else {
                    return false;
                };
                let snark = work.1.into();
                checked_snark_gossip(&work.0, &snark).is_ok() && eligible(&snark)
            }
            _ => false,
        }
    };
    rpc.publish.retain(allowed);
    if let Some(control) = &mut rpc.control {
        for ihave in &mut control.ihave {
            ihave.message_ids.retain(|id| {
                cache
                    .get_message_from_raw_message_id(id)
                    .is_some_and(|message| allowed(message.message()))
            });
        }
        control.ihave.retain(|ihave| !ihave.message_ids.is_empty());
        if control.ihave.is_empty()
            && control.iwant.is_empty()
            && control.graft.is_empty()
            && control.prune.is_empty()
        {
            rpc.control = None;
        }
    }
}

#[cfg(test)]
mod snark_admission_tests {
    use super::{
        p2p_network_pubsub_state::{
            P2pNetworkPubsubMessageCache, P2pNetworkPubsubMessageCacheMessage,
        },
        *,
    };
    use mina_p2p_messages::{rpc_kernel::PayloadBinprotReader, v2};

    fn work() -> mina_core::snark::Snark {
        mina_p2p_messages::rpc::GetCompletedSnarksV1::response_payload(
            &mut include_bytes!(
                "../../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin"
            )
            .as_slice(),
        )
        .unwrap()
        .unwrap()
        .0
        .into_iter()
        .next()
        .unwrap()
        .into()
    }

    #[test]
    fn advertised_statement_must_match_proofs_in_every_field_and_shape() {
        let snark = work();
        let mut statement = snark.statement();
        assert!(checked_snark_gossip(&statement, &snark).is_ok());
        let first = match &mut statement {
            v2::TransactionSnarkWorkStatementStableV2::One(a)
            | v2::TransactionSnarkWorkStatementStableV2::Two((a, _)) => a,
        };
        first.0.supply_increase.magnitude.0 =
            (first.0.supply_increase.magnitude.0.as_u64() + 1).into();
        assert!(checked_snark_gossip(&statement, &snark).is_err());
        let changed_shape = match snark.statement() {
            v2::TransactionSnarkWorkStatementStableV2::One(a) => {
                v2::TransactionSnarkWorkStatementStableV2::Two((a.clone(), a))
            }
            v2::TransactionSnarkWorkStatementStableV2::Two((a, _)) => {
                v2::TransactionSnarkWorkStatementStableV2::One(a)
            }
        };
        assert!(checked_snark_gossip(&changed_shape, &snark).is_err());
    }

    #[test]
    fn forwarding_selects_exact_validated_work_among_same_job_ids() {
        let snark = work();
        let peer_id = crate::identity::SecretKey::deterministic(4)
            .public_key()
            .peer_id();
        let id = |seqno| mina_core::p2p::P2pNetworkPubsubMessageCacheId {
            source: peer_id.to_libp2p_string().parse().unwrap(),
            seqno,
        };
        let mut cache = P2pNetworkPubsubMessageCache::default();
        let mut other = snark.clone();
        other.fee.0 = (other.fee.0.as_u64() + 1).into();
        let message = |work: mina_core::snark::Snark| {
            P2pNetworkPubsubMessageCacheMessage::PreValidatedSnark {
                job_id: work.job_id(),
                snark: Box::new(work),
                message: pb::Message::default(),
                peer_id,
                time: redux::Timestamp::ZERO,
            }
        };
        cache.map.insert(id(0), message(other.clone()));
        assert_eq!(cache.snark_message_id(&snark), None);
        cache.map.insert(id(1), message(snark.clone()));
        assert_eq!(cache.snark_message_id(&snark), Some(id(1)));
        assert_eq!(cache.snark_message_id(&other), Some(id(0)));
        // Equal fee is still not proof identity: change a public input without
        // changing the endpoint job ID.
        let mut wrong_proof = snark.clone();
        let mut proofs = wrong_proof.proofs.get().unwrap().as_ref().clone();
        let first = match &mut proofs {
            v2::TransactionSnarkWorkTStableV2Proofs::One(a)
            | v2::TransactionSnarkWorkTStableV2Proofs::Two((a, _)) => a,
        };
        first.0.statement.sok_digest = mina_p2p_messages::string::ByteString::from(vec![0; 32]);
        wrong_proof.proofs = proofs.into();
        assert_eq!(wrong_proof.job_id(), snark.job_id());
        assert_eq!(cache.snark_message_id(&wrong_proof), None);
    }
    #[test]
    fn queued_and_cached_gossip_is_rechecked_at_send_time() {
        let snark = work();
        let content = GossipNetMessageV2::SnarkPoolDiff {
            message: v2::NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(Box::new((
                snark.statement(),
                (&snark).try_into().unwrap(),
            ))),
            nonce: 0u64.into(),
        };
        let peer_id = crate::identity::SecretKey::deterministic(4)
            .public_key()
            .peer_id();
        let id = mina_core::p2p::P2pNetworkPubsubMessageCacheId {
            source: peer_id.to_libp2p_string().parse().unwrap(),
            seqno: 1,
        };
        let message = pb::Message {
            data: Some(encode_message(&content).unwrap()),
            ..Default::default()
        };
        let mut cache = P2pNetworkPubsubMessageCache::default();
        cache.map.insert(
            id,
            P2pNetworkPubsubMessageCacheMessage::Validated {
                message: message.clone(),
                peer_id,
                time: redux::Timestamp::ZERO,
            },
        );
        let rpc = pb::Rpc {
            publish: vec![message],
            control: Some(pb::ControlMessage {
                ihave: vec![pb::ControlIHave {
                    topic_id: Some("test".into()),
                    message_ids: vec![id.to_raw_bytes()],
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut current = rpc.clone();
        filter_snark_gossip(&mut current, &cache, |_| true);
        assert_eq!(current.publish.len(), 1);
        assert_eq!(current.control.as_ref().unwrap().ihave.len(), 1);
        let mut stale = rpc;
        filter_snark_gossip(&mut stale, &cache, |_| false);
        assert!(stale.publish.is_empty());
        assert!(stale.control.is_none());
    }
}
