use libp2p_identity::{DecodingError, PublicKey};

use super::super::pb;

use crate::{
    P2pCryptoService, P2pNetworkConnectionError, P2pNetworkPubsubAction, P2pNetworkSchedulerAction,
};

use super::P2pNetworkPubsubEffectfulAction;

#[derive(Debug, thiserror::Error)]
pub enum PubSubError {
    #[error("Message does not contain a signature.")]
    MissingSignature,
    #[error("Message does not contain a verifying key.")]
    MissingVerifyingKey,
    #[error("Failed to retrieve the originator's public key.")]
    OriginatorFailed,
    #[error("Message serialization failed.")]
    SerializationError,
    #[error("Message's signature is invalid.")]
    InvalidSignature,
    #[error("Message's key is not its publisher's.")]
    KeyNotPublishers,
}

impl P2pNetworkPubsubEffectfulAction {
    pub fn effects<Store, S>(self, _meta: &redux::ActionMeta, store: &mut Store)
    where
        Store: crate::P2pStore<S>,
        Store::Service: P2pCryptoService,
    {
        match self {
            P2pNetworkPubsubEffectfulAction::Sign { author, message } => {
                let mut publication = vec![];
                if prost::Message::encode(&message, &mut publication).is_err() {
                    store.dispatch(P2pNetworkPubsubAction::SignError {
                        author,
                        topic: message.topic,
                    });
                } else {
                    let signature = store.service().sign_publication(&publication).into();
                    store.dispatch(P2pNetworkPubsubAction::BroadcastSigned { signature });
                }
            }
            P2pNetworkPubsubEffectfulAction::ValidateIncomingMessages {
                peer_id,
                seen_limit,
                addr,
                messages,
            } => {
                let mut valid_messages = Vec::with_capacity(messages.len());

                for message in messages {
                    match validate_message(message, store) {
                        Ok(valid_msg) => valid_messages.push(valid_msg),
                        Err(error) => {
                            store.dispatch(P2pNetworkSchedulerAction::Error {
                                addr,
                                error: P2pNetworkConnectionError::PubSubError(error.to_string()),
                            });

                            return; // Early exit, no need to process the rest
                        }
                    }
                }

                // All good, we can continue with these validated messages
                for message in valid_messages {
                    store.dispatch(P2pNetworkPubsubAction::IncomingMessage {
                        peer_id,
                        message,
                        seen_limit,
                    });
                }
            }
        }
    }
}

fn validate_message<Store, S>(
    message: pb::Message,
    store: &mut Store,
) -> Result<pb::Message, PubSubError>
where
    Store: crate::P2pStore<S>,
    Store::Service: P2pCryptoService,
{
    let signature = message
        .signature
        .clone()
        .ok_or(PubSubError::MissingSignature)?;

    let pk = originator(&message)
        .map_err(|_| PubSubError::OriginatorFailed)?
        .ok_or(PubSubError::MissingVerifyingKey)?;
    if !key_is_publishers(&message, &pk) {
        return Err(PubSubError::KeyNotPublishers);
    }

    let mut message = message;
    let data = encode_without_signature_and_key(&mut message)
        .map_err(|_| PubSubError::SerializationError)?;

    if store.service().verify_publication(&pk, &data, &signature) {
        Ok(message)
    } else {
        Err(PubSubError::InvalidSignature)
    }
}

/// Whether `key` is the key of the message's claimed publisher, `from`. An
/// explicit key that isn't would let anyone publish under another node's ID,
/// and the ID names the message in our cache. go-libp2p refuses such a key.
/// go-libp2p-pubsub v0.9.3, as OCaml's libp2p helper pins it, `messagePubKey`:
/// <https://github.com/libp2p/go-libp2p-pubsub/blob/v0.9.3/sign.go>
pub fn key_is_publishers(message: &pb::Message, key: &PublicKey) -> bool {
    message
        .from
        .as_deref()
        .and_then(|from| libp2p_identity::PeerId::from_bytes(from).ok())
        .is_some_and(|from| from == key.to_peer_id())
}

pub fn originator(message: &pb::Message) -> Result<Option<PublicKey>, DecodingError> {
    message
        .key
        .as_deref()
        .or_else(|| message.from.as_deref().and_then(|f| f.get(2..)))
        .map(PublicKey::try_decode_protobuf)
        .transpose()
}

/// The reference to the message is mutable, but it is very important to keep the message the same
/// after this function is done.
pub fn encode_without_signature_and_key(
    message: &mut pb::Message,
) -> Result<Vec<u8>, prost::EncodeError> {
    // take the fields
    let signature = message.signature.take();
    let key = message.key.take();

    let mut data = vec![];
    let result = prost::Message::encode(&*message, &mut data);

    // put the fields back
    message.signature = signature;
    message.key = key;

    result.map(|()| data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p_identity::Keypair;

    fn signed_by(publisher: &Keypair, key: Option<&Keypair>) -> pb::Message {
        pb::Message {
            from: Some(publisher.public().to_peer_id().to_bytes()),
            data: Some(vec![1, 2, 3]),
            seqno: Some(1u64.to_be_bytes().to_vec()),
            topic: "coda/consensus-messages/0.0.1".to_owned(),
            signature: Some(vec![0; 64]),
            key: key.map(|k| k.public().encode_protobuf()),
        }
    }

    /// The key that signs must be the claimed publisher's, whether inlined
    /// in its ID or attached.
    #[test]
    fn an_attached_key_must_be_the_publishers() {
        let publisher = Keypair::generate_ed25519();
        let other = Keypair::generate_ed25519();
        for (message, ok) in [
            (signed_by(&publisher, None), true),
            (signed_by(&publisher, Some(&publisher)), true),
            (signed_by(&publisher, Some(&other)), false),
        ] {
            let key = originator(&message).unwrap().unwrap();
            assert_eq!(key_is_publishers(&message, &key), ok);
        }
        let mut anonymous = signed_by(&publisher, Some(&publisher));
        anonymous.from = None;
        let key = originator(&anonymous).unwrap().unwrap();
        assert!(!key_is_publishers(&anonymous, &key));
    }
}
