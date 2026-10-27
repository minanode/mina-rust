use ledger::FpExt;
use mina_p2p_messages::bigint::BigInt;
use mina_signer::Signature;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use super::{
    RpcNodeStatus, RpcNodeStatusSnarkPool, RpcNodeStatusTransactionPool,
    RpcNodeStatusTransitionFrontier,
};
use crate::{p2p::PeerId, stats::block_producer::BlockProductionAttempt};
use mina_node_account::{AccountPublicKey, AccountSecretKey};

/// Matches the representation used by o1js where each field is a string
/// containing a decimal representation of the field.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignatureJson {
    pub field: String,
    pub scalar: String,
}

impl From<Signature> for SignatureJson {
    fn from(sig: Signature) -> Self {
        Self {
            field: sig.rx.to_decimal(),
            scalar: sig.s.to_decimal(),
        }
    }
}

impl TryInto<Signature> for SignatureJson {
    type Error = String;

    fn try_into(self) -> Result<Signature, Self::Error> {
        let rx = BigInt::from_decimal(&self.field)
            .map_err(|_| "Failed to parse decimals as BigInt")?
            .try_into()
            .map_err(|_| "Failed to convert rx BigInt to field element")?;
        let s = BigInt::from_decimal(&self.scalar)
            .map_err(|_| "Failed to parse decimals as BigInt")?
            .try_into()
            .map_err(|_| "Failed to convert rx BigInt to field element")?;

        Ok(Signature::new(rx, s))
    }
}

/// A signed heartbeat message from a node
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignedNodeHeartbeat {
    pub version: u8,
    /// base64 encoded json of the payload
    pub payload: String,
    pub submitter: AccountPublicKey,
    pub signature: SignatureJson,
}

impl SignedNodeHeartbeat {
    /// Verifies that the signature is valid for this heartbeat
    pub fn verify_signature(&self) -> bool {
        use blake2::digest::{Update, VariableOutput};
        use mina_signer::{CompressedPubKey, PubKey, Signer};

        let signature = match self.signature.clone().try_into() {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        let pk: CompressedPubKey = match self.submitter.clone().try_into() {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        let pk = match PubKey::from_address(&pk.into_address()) {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        // Calculate digest from payload
        let mut hasher = blake2::Blake2bVar::new(32).expect("Invalid Blake2bVar output size");
        let mut blake2_hash = [0u8; 32];
        hasher.update(self.payload.as_bytes());
        hasher.finalize_variable(&mut blake2_hash).unwrap();

        let digest = NodeHeartbeatPayloadDigest(blake2_hash);
        let mut signer = mina_signer::create_legacy::<NodeHeartbeatPayloadDigest>(
            mina_signer::NetworkId::TESTNET,
        );

        signer.verify(&signature, &pk, &digest)
    }
}

/// Node heartbeat
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NodeHeartbeat {
    pub status: NodeStatus,
    pub node_timestamp: Timestamp,
    pub peer_id: PeerId,
    // binprot+base64 encoded block header
    pub last_produced_block_info: Option<ProducedBlockInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProducedBlockInfo {
    pub height: u32,
    pub global_slot: u32,
    pub hash: String,
    pub base64_encoded_header: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NodeStatus {
    pub chain_id: Option<String>,
    pub transition_frontier: RpcNodeStatusTransitionFrontier,
    pub peers_count: u32,
    pub snark_pool: RpcNodeStatusSnarkPool,
    pub transaction_pool: RpcNodeStatusTransactionPool,
    pub current_block_production_attempt: Option<BlockProductionAttempt>,
}

impl From<RpcNodeStatus> for NodeStatus {
    fn from(status: RpcNodeStatus) -> Self {
        Self {
            chain_id: status.chain_id,
            transition_frontier: status.transition_frontier,
            peers_count: status.peers.len() as u32,
            snark_pool: status.snark_pool,
            transaction_pool: status.transaction_pool,
            current_block_production_attempt: status.current_block_production_attempt,
        }
    }
}

/// The legacy random oracle input of a string, as OCaml's `String_sign` (and
/// mina-signer's `signMessage`) builds it: the bits of each byte, most
/// significant first. `append_bytes` takes them least significant first,
/// hence the reversal.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/string_sign/string_sign.ml#L43-L54>
fn string_to_roinput(s: &[u8]) -> mina_hasher::ROInput {
    let reversed: Vec<u8> = s.iter().map(|b| b.reverse_bits()).collect();
    mina_hasher::ROInput::new().append_bytes(&reversed)
}

fn signature_domain_string(network_id: mina_signer::NetworkId) -> Option<String> {
    match network_id {
        mina_signer::NetworkId::MAINNET => mina_core::network::mainnet::SIGNATURE_PREFIX,
        mina_signer::NetworkId::TESTNET => mina_core::network::devnet::SIGNATURE_PREFIX,
    }
    .to_string()
    .into()
}

/// Blake2b hash of the encoded heartbeat payload
///
/// It is signed as a string, its 64 lowercase hex digits.
#[derive(Clone, Debug)]
pub struct NodeHeartbeatPayloadDigest([u8; 32]);

impl mina_hasher::Hashable for NodeHeartbeatPayloadDigest {
    type D = mina_signer::NetworkId;

    fn to_roinput(&self) -> mina_hasher::ROInput {
        let mut hex = [0u8; 64];
        hex::encode_to_slice(self.0, &mut hex).unwrap();
        string_to_roinput(&hex)
    }

    fn domain_string(network_id: Self::D) -> Option<String> {
        signature_domain_string(network_id)
    }
}

impl NodeHeartbeat {
    const CURRENT_VERSION: u8 = 1;

    /// Creates base64 encoded payload and its Blake2b digest
    fn payload_and_digest(&self) -> (String, NodeHeartbeatPayloadDigest) {
        use base64::{engine::general_purpose::URL_SAFE, Engine as _};
        use blake2::{
            digest::{Update, VariableOutput},
            Blake2bVar,
        };

        let payload = serde_json::to_string(self).unwrap();
        let encoded_payload = URL_SAFE.encode(&payload);

        let mut hasher = Blake2bVar::new(32).expect("Invalid Blake2bVar output size");
        let mut blake2_hash = [0u8; 32];

        hasher.update(encoded_payload.as_bytes());
        hasher.finalize_variable(&mut blake2_hash).unwrap();

        (encoded_payload, NodeHeartbeatPayloadDigest(blake2_hash))
    }

    /// Signs the heartbeat using the provided secret key
    pub fn sign(&self, secret_key: &AccountSecretKey) -> SignedNodeHeartbeat {
        let (payload, digest) = self.payload_and_digest();
        let submitter = secret_key.public_key();

        let signature = {
            use mina_signer::{Keypair, Signer};
            let mut signer = mina_signer::create_legacy::<NodeHeartbeatPayloadDigest>(
                mina_signer::NetworkId::TESTNET,
            );
            let kp = Keypair::from(secret_key.clone());

            let signature = signer.sign(&kp, &digest, mina_signer::NonceMode::Legacy);
            signature.into()
        };

        SignedNodeHeartbeat {
            version: Self::CURRENT_VERSION,
            payload,
            submitter,
            signature,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {

    use crate::rpc::{
        RpcNodeStatusSnarkPool, RpcNodeStatusTransactionPool, RpcNodeStatusTransitionFrontier,
        RpcNodeStatusTransitionFrontierSync,
    };

    use super::*;
    use redux::Timestamp;

    /// The payload is the JSON below, URL-safe base64. The signature is
    /// mina-signer's `signMessage` (testnet) of the payload's Blake2b-256 hex
    /// digest, mina-signer being what the heartbeat receiver verified with; it
    /// matches OCaml's `String_sign` regression vectors (see
    /// `test_string_signing_matches_ocaml`). The test key is
    /// `AccountSecretKey::deterministic(0)`. Refreshed for R24g's `root` field
    /// using mina-signer 3.1.0 after checking both OCaml String_sign vectors
    /// and the previous heartbeat signature (wave-3a-integration.md).
    #[test]
    fn test_heartbeat_signing() {
        use base64::{engine::general_purpose::URL_SAFE, Engine as _};

        let heartbeat = create_test_heartbeat();
        let secret_key = AccountSecretKey::deterministic(0);
        let signed = heartbeat.sign(&secret_key);

        assert_eq!(
            signed.submitter.to_string(),
            "B62qnLjgW4LAnrxkcdLc7Snb49qx6aP5qsmPsp6ueZN4XPMC621cqGc"
        );
        let json = URL_SAFE.decode(&signed.payload).unwrap();
        assert_eq!(
            std::str::from_utf8(&json).unwrap(),
            concat!(
                r#"{"status":{"chain_id":null,"transition_frontier":{"root":null,"best_tip":null,"#,
                r#""sync":{"time":null,"status":"Synced","phase":"Running","target":null}},"#,
                r#""peers_count":10,"snark_pool":{"total_jobs":0,"snarks":0},"#,
                r#""transaction_pool":{"transactions":0,"transactions_for_propagation":0,"#,
                r#""transaction_candidates":0},"current_block_production_attempt":null},"#,
                r#""node_timestamp":0,"#,
                r#""peer_id":"2bEgBrPTzL8wov2D4Kz34WVLCxR4uCarsBmHYXWKQA5wvBQzd9H","#,
                r#""last_produced_block_info":null}"#
            )
        );
        assert_eq!(&signed.payload, "eyJzdGF0dXMiOnsiY2hhaW5faWQiOm51bGwsInRyYW5zaXRpb25fZnJvbnRpZXIiOnsicm9vdCI6bnVsbCwiYmVzdF90aXAiOm51bGwsInN5bmMiOnsidGltZSI6bnVsbCwic3RhdHVzIjoiU3luY2VkIiwicGhhc2UiOiJSdW5uaW5nIiwidGFyZ2V0IjpudWxsfX0sInBlZXJzX2NvdW50IjoxMCwic25hcmtfcG9vbCI6eyJ0b3RhbF9qb2JzIjowLCJzbmFya3MiOjB9LCJ0cmFuc2FjdGlvbl9wb29sIjp7InRyYW5zYWN0aW9ucyI6MCwidHJhbnNhY3Rpb25zX2Zvcl9wcm9wYWdhdGlvbiI6MCwidHJhbnNhY3Rpb25fY2FuZGlkYXRlcyI6MH0sImN1cnJlbnRfYmxvY2tfcHJvZHVjdGlvbl9hdHRlbXB0IjpudWxsfSwibm9kZV90aW1lc3RhbXAiOjAsInBlZXJfaWQiOiIyYkVnQnJQVHpMOHdvdjJENEt6MzRXVkxDeFI0dUNhcnNCbUhZWFdLUUE1d3ZCUXpkOUgiLCJsYXN0X3Byb2R1Y2VkX2Jsb2NrX2luZm8iOm51bGx9");
        assert_eq!(
            &signed.signature.field,
            "21591907503138568507511927066594681457669824843755395181133460748356312052825"
        );
        assert_eq!(
            &signed.signature.scalar,
            "20970727517380612997319779643799431855146986630623614310402581414217508614509"
        );
        assert!(signed.verify_signature());
    }

    /// A string signed as `String_sign` signs it.
    #[derive(Clone)]
    struct LegacyString(&'static str);

    impl mina_hasher::Hashable for LegacyString {
        type D = mina_signer::NetworkId;

        fn to_roinput(&self) -> mina_hasher::ROInput {
            super::string_to_roinput(self.0.as_bytes())
        }

        fn domain_string(network_id: Self::D) -> Option<String> {
            super::signature_domain_string(network_id)
        }
    }

    /// OCaml 4.0.2's regression vectors for `String_sign.sign`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/string_sign/tests/test_string_sign.ml#L195-L230>
    #[test]
    fn test_string_signing_matches_ocaml() {
        use mina_signer::{Keypair, NetworkId, ScalarField, SecKey, Signer};
        use std::str::FromStr;

        let secret = ScalarField::from_str(
            "28948022309329048855892746252171976963363056481941560715954676764349967630337",
        )
        .unwrap();
        let keypair = Keypair::try_from(SecKey::new(secret)).unwrap();
        let message = LegacyString("Bitcoin: A Peer-to-Peer Electronic Cash System");
        for (network, field, scalar) in [
            (
                NetworkId::MAINNET,
                "10098659636052751402960513659673534058318534694837361110423199873688986365908",
                "7844530585816769124362208605281765856859093320029640751494040544682079710450",
            ),
            (
                NetworkId::TESTNET,
                "19528641019288828403170135634909672400537532147682562327700497341129840775560",
                "24640829174932073706335857230587954505385529622139330517748682166416541732513",
            ),
        ] {
            let mut signer = mina_signer::create_legacy::<LegacyString>(network);
            let signature: SignatureJson = signer
                .sign(&keypair, &message, mina_signer::NonceMode::Legacy)
                .into();
            assert_eq!(signature.field, field);
            assert_eq!(signature.scalar, scalar);
        }
    }

    #[test]
    fn test_heartbeat_signature_deterministic() {
        let heartbeat = create_test_heartbeat();
        let secret_key = AccountSecretKey::deterministic(0);

        let signed1 = heartbeat.sign(&secret_key);
        let signed2 = heartbeat.sign(&secret_key);

        assert_eq!(signed1.payload, signed2.payload);
        assert_eq!(signed1.signature, signed2.signature);
    }

    #[test]
    fn test_heartbeat_different_keys_different_sigs() {
        let heartbeat = create_test_heartbeat();
        let sk1 = AccountSecretKey::deterministic(0);
        let sk2 = AccountSecretKey::deterministic(1);

        let signed1 = heartbeat.sign(&sk1);
        let signed2 = heartbeat.sign(&sk2);

        assert_eq!(signed1.payload, signed2.payload);
        assert_ne!(signed1.signature, signed2.signature);
        assert_ne!(signed1.submitter, signed2.submitter);
    }

    fn create_test_heartbeat() -> NodeHeartbeat {
        NodeHeartbeat {
            status: NodeStatus {
                chain_id: None,
                transition_frontier: RpcNodeStatusTransitionFrontier {
                    root: None,
                    best_tip: None,
                    sync: RpcNodeStatusTransitionFrontierSync {
                        time: None,
                        status: "Synced".to_string(),
                        phase: "Running".to_string(),
                        target: None,
                    },
                },
                peers_count: 10,
                snark_pool: RpcNodeStatusSnarkPool::default(),
                transaction_pool: RpcNodeStatusTransactionPool::default(),
                current_block_production_attempt: None,
            },
            node_timestamp: Timestamp::ZERO,
            peer_id: "2bEgBrPTzL8wov2D4Kz34WVLCxR4uCarsBmHYXWKQA5wvBQzd9H"
                .parse()
                .unwrap(),
            last_produced_block_info: None,
        }
    }
}
